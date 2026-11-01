use std::fs;
use std::path::Path;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, QueryServiceStatus, StartServiceW, SERVICE_AUTO_START,
    SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_STOP, SERVICE_ERROR_NORMAL, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_START, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STOP,
    SERVICE_STOPPED, SERVICE_WIN32_OWN_PROCESS,
};

use super::config::{
    self, generate_token, installed_service_path, remove_config, save_config, AgentConfig,
    PROTOCOL_VERSION, SERVICE_ARG, SERVICE_DISPLAY_NAME, SERVICE_NAME,
};
use super::identity::{current_user_sid, is_elevated, wide};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

const SC_MANAGER_CONNECT: u32 = 0x0001;
const SC_MANAGER_CREATE_SERVICE: u32 = 0x0002;
const DELETE_ACCESS: u32 = 0x0001_0000;
const ERROR_SERVICE_ALREADY_RUNNING: u32 = 1056;

pub fn launch_elevated(argument: &str) -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("读取应用路径失败：{error}"))?;
    let file = wide(&executable.to_string_lossy());
    let parameters = wide(argument);
    let verb = wide("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).map_err(|error| {
            let code = error.code().0 as u32 & 0xFFFF;
            if code == 1223 {
                "已取消管理员授权".to_string()
            } else {
                format!("启动提权安装程序失败：{error}")
            }
        })?;
    }
    if info.hProcess.is_invalid() {
        return Ok(());
    }
    unsafe {
        let _ = WaitForSingleObject(info.hProcess, INFINITE);
        let mut exit_code = 0_u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut exit_code);
        let _ = CloseHandle(info.hProcess);
        if exit_code != 0 {
            return Err(format!("提权安装程序退出码 {exit_code}，详情见服务日志"));
        }
    }
    Ok(())
}

pub fn run_install() -> i32 {
    if !is_elevated() {
        config::log("安装输入服务需要管理员权限");
        return 1;
    }
    let owner_sid = owner_sid_from_args()
        .or_else(current_user_sid)
        .unwrap_or_default();
    match install(&owner_sid) {
        Ok(()) => {
            config::log("输入服务安装完成");
            0
        }
        Err(error) => {
            config::log(format!("输入服务安装失败：{error}"));
            1
        }
    }
}

pub fn run_uninstall() -> i32 {
    if !is_elevated() {
        config::log("卸载输入服务需要管理员权限");
        return 1;
    }
    match uninstall() {
        Ok(()) => {
            config::log("输入服务已卸载");
            0
        }
        Err(error) => {
            config::log(format!("输入服务卸载失败：{error}"));
            1
        }
    }
}

fn owner_sid_from_args() -> Option<String> {
    let mut args = std::env::args().skip_while(|arg| arg != config::INSTALL_ARG);
    args.next()?;
    args.next().filter(|value| value.starts_with("S-1-"))
}

fn install(owner_sid: &str) -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("读取应用路径失败：{error}"))?;
    let source = executable.display().to_string();
    let target = installed_service_path();
    let app_path = source.clone();
    let user_sid = if owner_sid.is_empty() {
        current_user_sid().ok_or_else(|| "无法读取当前用户 SID".to_string())?
    } else {
        owner_sid.to_string()
    };
    stop_and_delete_service();
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败：{error}", parent.display()))?;
    }
    if !same_file(&executable, &target) {
        fs::copy(&executable, &target)
            .map_err(|error| format!("复制服务程序到 {} 失败：{error}", target.display()))?;
    }
    let config = AgentConfig {
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: PROTOCOL_VERSION,
        token: generate_token(),
        user_sid,
        app_path,
        service_path: target.display().to_string(),
        pipe_name: config::PIPE_NAME.to_string(),
    };
    save_config(&config)?;
    register_service(&target)?;
    Ok(())
}

fn uninstall() -> Result<(), String> {
    stop_and_delete_service();
    remove_config();
    let target = installed_service_path();
    if target.exists() {
        fs::remove_file(&target)
            .map_err(|error| format!("删除 {} 失败：{error}", target.display()))?;
    }
    if let Some(parent) = target.parent() {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}

pub fn service_state() -> Option<SERVICE_STATUS_CURRENT_STATE> {
    unsafe {
        let manager = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).ok()?;
        let name = wide(SERVICE_NAME);
        let service = OpenServiceW(manager, PCWSTR(name.as_ptr()), SERVICE_QUERY_STATUS).ok();
        let state = service.map(|service| {
            let mut status = SERVICE_STATUS::default();
            let state = QueryServiceStatus(service, &mut status)
                .ok()
                .map(|()| status.dwCurrentState);
            let _ = CloseServiceHandle(service);
            state
        });
        let _ = CloseServiceHandle(manager);
        state.flatten()
    }
}

pub fn is_running() -> bool {
    service_state() == Some(SERVICE_RUNNING)
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn stop_and_delete_service() {
    unsafe {
        let Ok(manager) = OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        ) else {
            return;
        };
        let name = wide(SERVICE_NAME);
        if let Ok(service) = OpenServiceW(
            manager,
            PCWSTR(name.as_ptr()),
            SERVICE_STOP | SERVICE_QUERY_STATUS | DELETE_ACCESS | SERVICE_CHANGE_CONFIG,
        ) {
            let mut status = SERVICE_STATUS::default();
            let _ = ControlService(service, SERVICE_CONTROL_STOP, &mut status);
            for _ in 0..20 {
                let mut current = SERVICE_STATUS::default();
                if QueryServiceStatus(service, &mut current).is_err()
                    || current.dwCurrentState == SERVICE_STOPPED
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            let _ = DeleteService(service);
            let _ = CloseServiceHandle(service);
        }
        let _ = CloseServiceHandle(manager);
    }
}

fn register_service(executable: &Path) -> Result<(), String> {
    let binary = wide(&format!("\"{}\" {}", executable.display(), SERVICE_ARG));
    let name = wide(SERVICE_NAME);
    let display = wide(SERVICE_DISPLAY_NAME);
    unsafe {
        let manager = OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        )
        .map_err(|error| format!("打开服务管理器失败：{error}"))?;
        let service = OpenServiceW(
            manager,
            PCWSTR(name.as_ptr()),
            SERVICE_CHANGE_CONFIG | SERVICE_QUERY_STATUS | SERVICE_START,
        );
        let service = match service {
            Ok(service) => {
                ChangeServiceConfigW(
                    service,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(binary.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR(display.as_ptr()),
                )
                .map_err(|error| format!("更新服务配置失败：{error}"))?;
                service
            }
            Err(_) => CreateServiceW(
                manager,
                PCWSTR(name.as_ptr()),
                PCWSTR(display.as_ptr()),
                0x000F_01FF,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                PCWSTR(binary.as_ptr()),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            )
            .map_err(|error| format!("创建输入服务失败：{error}"))?,
        };
        let start = StartServiceW(service, None);
        if let Err(error) = start {
            let code = error.code().0 as u32 & 0xFFFF;
            if code != ERROR_SERVICE_ALREADY_RUNNING {
                let _ = CloseServiceHandle(service);
                let _ = CloseServiceHandle(manager);
                return Err(format!("启动输入服务失败：{error}"));
            }
        }
        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(manager);
    }
    Ok(())
}
