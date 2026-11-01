use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::thread;
use std::time::Duration;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenPrimary, TokenSessionId,
    TOKEN_ALL_ACCESS, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY,
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOPPED,
    SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, TerminateProcess,
    WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW,
};

use super::config::{self, installed_service_path, load_config};
use super::identity::{enable_privilege, wide};

static STOP: AtomicBool = AtomicBool::new(false);
static STATUS_HANDLE: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(null_mut());

pub fn run_service() {
    let mut name = wide(config::SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR::null(),
            lpServiceProc: None,
        },
    ];
    unsafe {
        let _ = StartServiceCtrlDispatcherW(table.as_ptr());
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = wide(config::SERVICE_NAME);
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(service_handler), None)
    };
    let Ok(handle) = handle else {
        config::log("注册输入服务控制处理器失败");
        return;
    };
    STATUS_HANDLE.store(handle.0, Ordering::SeqCst);
    set_status(SERVICE_START_PENDING, SERVICE_ACCEPT_STOP, 3_000);
    config::log("NexusCat 输入服务启动");
    let worker = thread::Builder::new()
        .name("nc-input-service-monitor".into())
        .spawn(monitor);
    set_status(SERVICE_RUNNING, SERVICE_ACCEPT_STOP, 0);
    let Ok(worker) = worker else {
        config::log("启动输入服务监控线程失败");
        set_status(SERVICE_STOPPED, 0, 0);
        return;
    };
    while !STOP.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(500));
    }
    set_status(SERVICE_STOP_PENDING, 0, 5_000);
    let _ = worker.join();
    set_status(SERVICE_STOPPED, 0, 0);
    config::log("NexusCat 输入服务已停止");
}

unsafe extern "system" fn service_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    if control == SERVICE_CONTROL_STOP {
        STOP.store(true, Ordering::SeqCst);
    }
    0
}

fn set_status(state: SERVICE_STATUS_CURRENT_STATE, accepted: u32, wait_hint: u32) {
    let handle = STATUS_HANDLE.load(Ordering::SeqCst);
    if handle.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    unsafe {
        let _ = SetServiceStatus(SERVICE_STATUS_HANDLE(handle), &status);
    }
}

fn monitor() {
    let mut agent: Option<AgentProcess> = None;
    let mut session = u32::MAX;
    while !STOP.load(Ordering::SeqCst) {
        let active = unsafe { WTSGetActiveConsoleSessionId() };
        if active == u32::MAX {
            if let Some(process) = agent.take() {
                process.stop();
            }
            session = u32::MAX;
            thread::sleep(Duration::from_secs(2));
            continue;
        }
        if session != active {
            if let Some(process) = agent.take() {
                process.stop();
            }
            session = active;
        }
        if agent.as_ref().is_none_or(|process| !process.alive()) {
            if let Some(process) = agent.take() {
                process.stop();
            }
            agent = launch_agent(active);
        }
        thread::sleep(Duration::from_secs(2));
    }
    if let Some(process) = agent.take() {
        process.stop();
    }
}

struct AgentProcess {
    handle: HANDLE,
}

impl AgentProcess {
    fn alive(&self) -> bool {
        unsafe { WaitForSingleObject(self.handle, 0) == WAIT_TIMEOUT }
    }

    fn stop(mut self) {
        unsafe {
            let _ = TerminateProcess(self.handle, 0);
            let handle = std::mem::take(&mut self.handle);
            let _ = CloseHandle(handle);
        }
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

fn launch_agent(session_id: u32) -> Option<AgentProcess> {
    match load_config() {
        Ok(config) if config.is_current() => {}
        _ => {
            config::log("输入服务尚未安装或配置版本不匹配，暂不启动会话代理");
            return None;
        }
    }
    let service_path = installed_service_path();
    if !service_path.exists() {
        config::log(format!("找不到输入服务程序：{}", service_path.display()));
        return None;
    }
    for privilege in [
        "SeTcbPrivilege",
        "SeAssignPrimaryTokenPrivilege",
        "SeIncreaseQuotaPrivilege",
    ] {
        if let Err(error) = enable_privilege(privilege) {
            config::log(format!("启用 {privilege} 失败：{error}"));
        }
    }

    unsafe {
        let mut token = HANDLE::default();
        if let Err(error) = OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
            &mut token,
        ) {
            config::log(format!("打开服务令牌失败：{error}"));
            return None;
        }
        let mut primary = HANDLE::default();
        let duplicate = DuplicateTokenEx(
            token,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        );
        let _ = CloseHandle(token);
        if let Err(error) = duplicate {
            config::log(format!("复制服务令牌失败：{error}"));
            return None;
        }
        let session_bytes = session_id.to_le_bytes();
        if let Err(error) = SetTokenInformation(
            primary,
            TokenSessionId,
            session_bytes.as_ptr().cast(),
            std::mem::size_of::<u32>() as u32,
        ) {
            config::log(format!("设置代理会话 ID 失败：{error}"));
            let _ = CloseHandle(primary);
            return None;
        }

        let executable = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                config::log(format!("读取服务程序路径失败：{error}"));
                let _ = CloseHandle(primary);
                return None;
            }
        };
        let executable_wide = wide(&executable.to_string_lossy());
        let mut command_line = wide(&format!(
            "\"{}\" {}",
            executable.display(),
            config::AGENT_ARG
        ));
        let desktop = wide("winsta0\\default");
        let working_directory = executable
            .parent()
            .map(|path| wide(&path.to_string_lossy()))
            .unwrap_or_else(|| wide("C:\\"));
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_ptr() as *mut u16),
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        let result = CreateProcessAsUserW(
            Some(primary),
            PCWSTR(executable_wide.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW,
            None,
            PCWSTR(working_directory.as_ptr()),
            &startup,
            &mut process,
        );
        let _ = CloseHandle(primary);
        match result {
            Ok(()) => {
                let _ = CloseHandle(process.hThread);
                config::log(format!(
                    "已在会话 {session_id} 启动输入服务代理 pid={}",
                    process.dwProcessId
                ));
                Some(AgentProcess {
                    handle: process.hProcess,
                })
            }
            Err(error) => {
                config::log(format!("启动会话代理失败：{error}"));
                None
            }
        }
    }
}
