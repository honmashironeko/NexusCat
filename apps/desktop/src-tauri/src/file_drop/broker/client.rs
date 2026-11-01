use super::proto::{self, Command, Event as WireEvent, Hello, Welcome, PROTOCOL_VERSION};
use crate::runtime::{AppEvent, AppState, BrokerEvent};
use rand::RngCore;
use std::fs::File;
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::path::Path;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSidToSidW, SetEntriesInAclW, EXPLICIT_ACCESS_W, GRANT_ACCESS, TRUSTEE_IS_SID,
    TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, DuplicateTokenEx, InitializeSecurityDescriptor, LookupPrivilegeValueW,
    SetSecurityDescriptorDacl, TokenPrimary, ACE_FLAGS, ACL, LUID_AND_ATTRIBUTES,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SE_IMPERSONATE_NAME,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ALL_ACCESS, TOKEN_DUPLICATE,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    CreateProcessWithTokenW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    CREATE_PROCESS_LOGON_FLAGS, LOGON_WITH_PROFILE, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

const PIPE_BUFFER_SIZE: u32 = 64 * 1024;
const CONNECT_WATCHDOG: Duration = Duration::from_secs(10);
const SYSTEM_SID: &str = "S-1-5-18";
const SECURITY_DESCRIPTOR_REVISION: u32 = 1;

static CLIENT: OnceLock<Mutex<Option<Client>>> = OnceLock::new();
static STARTED: OnceLock<()> = OnceLock::new();

struct Client {
    commands: Sender<Command>,
    ready: Arc<AtomicBool>,
}

pub(crate) fn start(state: Arc<AppState>) {
    STARTED.get_or_init(|| {
        if let Err(error) = start_inner(&state) {
            tracing::warn!(%error, "启动文件拖放代理失败");
            let _ = state
                .events
                .send(AppEvent::FileDropBroker(BrokerEvent::Unavailable {
                    reason: error,
                }));
        }
    });
}

pub(crate) fn show(x: i32, y: i32, width: i32, height: i32) -> bool {
    let guard = client().lock().expect("文件拖放代理客户端锁被污染");
    let Some(client) = guard.as_ref() else {
        return false;
    };
    if !client.ready.load(Ordering::SeqCst) {
        return false;
    }
    let sent = client
        .commands
        .send(Command::Show {
            x,
            y,
            width,
            height,
        })
        .is_ok();
    if !sent {
        client.ready.store(false, Ordering::SeqCst);
    }
    sent
}

pub(crate) fn hide() {
    let guard = client().lock().expect("文件拖放代理客户端锁被污染");
    let Some(client) = guard.as_ref() else {
        return;
    };
    if !client.ready.load(Ordering::SeqCst) {
        return;
    }
    let _ = client.commands.send(Command::Hide);
}

pub(crate) fn available() -> bool {
    let guard = client().lock().expect("文件拖放代理客户端锁被污染");
    guard
        .as_ref()
        .map(|client| client.ready.load(Ordering::SeqCst))
        .unwrap_or(false)
}

fn client() -> &'static Mutex<Option<Client>> {
    CLIENT.get_or_init(|| Mutex::new(None))
}

fn start_inner(state: &Arc<AppState>) -> Result<(), String> {
    let shell = ShellSession::resolve()?;
    let pipe_name = format!(
        r"\\.\pipe\NexusCat.FileDropBroker.{}.{}",
        std::process::id(),
        random_hex(16)
    );
    let token = random_hex(32);

    let mut security = PipeSecurity::new(&shell.user_sid)?;
    let attributes = security.attributes();
    let name = super::wide(&pipe_name);
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            0,
            Some(&attributes),
        )
    };
    if handle.is_invalid() {
        return Err(format!(
            "创建文件拖放代理管道失败：{}",
            std::io::Error::last_os_error()
        ));
    }
    let server = unsafe { File::from_raw_handle(handle.0 as RawHandle) };
    drop(security);

    let ready = Arc::new(AtomicBool::new(false));
    let (commands, commands_rx) = mpsc::channel::<Command>();
    {
        let mut guard = client().lock().expect("文件拖放代理客户端锁被污染");
        *guard = Some(Client {
            commands,
            ready: Arc::clone(&ready),
        });
    }

    let exe =
        std::env::current_exe().map_err(|error| format!("读取当前可执行文件失败：{error}"))?;
    let broker_pid = spawn_broker(&shell, &exe, &pipe_name, &token)?;

    let accept_ready = Arc::clone(&ready);
    let accept_state = Arc::clone(state);
    let accept_token = token.clone();
    thread::Builder::new()
        .name("nc-drop-broker-accept".into())
        .spawn(move || {
            accept_loop(
                server,
                commands_rx,
                accept_ready,
                accept_state,
                accept_token,
                broker_pid,
            )
        })
        .map_err(|error| format!("启动文件拖放代理连接线程失败：{error}"))?;

    let watchdog_ready = Arc::clone(&ready);
    let watchdog_events = state.events.clone();
    thread::Builder::new()
        .name("nc-drop-broker-watchdog".into())
        .spawn(move || {
            thread::sleep(CONNECT_WATCHDOG);
            if !watchdog_ready.load(Ordering::SeqCst) {
                let _ = watchdog_events.send(AppEvent::FileDropBroker(BrokerEvent::Unavailable {
                    reason: "文件拖放代理未在限定时间内连接".into(),
                }));
            }
        })
        .map_err(|error| format!("启动文件拖放代理看门狗失败：{error}"))?;

    Ok(())
}

fn accept_loop(
    server: File,
    commands: mpsc::Receiver<Command>,
    ready: Arc<AtomicBool>,
    state: Arc<AppState>,
    token: String,
    expected_pid: u32,
) {
    if let Err(error) = handshake(&server, &token, expected_pid) {
        tracing::warn!(%error, "文件拖放代理握手失败");
        return;
    }
    let reader = match server.try_clone() {
        Ok(reader) => reader,
        Err(error) => {
            tracing::warn!(%error, "复制文件拖放代理管道句柄失败");
            return;
        }
    };
    let reader_ready = Arc::clone(&ready);
    if let Err(error) = thread::Builder::new()
        .name("nc-drop-broker-events".into())
        .spawn(move || reader_loop(reader, state, reader_ready))
    {
        tracing::warn!(%error, "启动文件拖放代理事件线程失败");
        return;
    }
    ready.store(true, Ordering::SeqCst);
    tracing::info!("文件拖放代理已连接");

    let mut writer = server;
    while let Ok(command) = commands.recv() {
        if proto::write_frame(&mut writer, &command).is_err() {
            break;
        }
    }
    ready.store(false, Ordering::SeqCst);
}

fn handshake(server: &File, token: &str, expected_pid: u32) -> Result<(), String> {
    match unsafe { ConnectNamedPipe(HANDLE(server.as_raw_handle()), None) } {
        Ok(()) => {}
        Err(error) if (error.code().0 as u32 & 0xFFFF) == ERROR_PIPE_CONNECTED.0 => {}
        Err(error) => return Err(format!("等待文件拖放代理连接失败：{error}")),
    }
    let mut client_pid = 0_u32;
    unsafe { GetNamedPipeClientProcessId(HANDLE(server.as_raw_handle()), &mut client_pid) }
        .map_err(|error| format!("读取文件拖放代理进程 ID 失败：{error}"))?;
    if client_pid != expected_pid {
        return Err(format!(
            "文件拖放代理进程不匹配：{client_pid} != {expected_pid}"
        ));
    }
    let mut input = server;
    let hello: Hello = proto::read_frame(&mut input).map_err(|error| error.to_string())?;
    if hello.token != token {
        return Err("文件拖放代理令牌不匹配".into());
    }
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "文件拖放代理协议版本不一致：{} != {}",
            hello.protocol_version, PROTOCOL_VERSION
        ));
    }
    let mut output = server;
    proto::write_frame(
        &mut output,
        &Welcome {
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .map_err(|error| error.to_string())
}

fn reader_loop(mut file: File, state: Arc<AppState>, ready: Arc<AtomicBool>) {
    loop {
        match proto::read_frame::<_, WireEvent>(&mut file) {
            Ok(event) => {
                if state
                    .events
                    .send(AppEvent::FileDropBroker(convert(event)))
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => {
                ready.store(false, Ordering::SeqCst);
                let _ = state
                    .events
                    .send(AppEvent::FileDropBroker(BrokerEvent::Disconnected {
                        reason: "文件拖放代理连接已断开".into(),
                    }));
                break;
            }
        }
    }
    ready.store(false, Ordering::SeqCst);
}

fn convert(event: WireEvent) -> BrokerEvent {
    match event {
        WireEvent::Enter { count, x, y } => BrokerEvent::Enter { count, x, y },
        WireEvent::Over { x, y } => BrokerEvent::Over { x, y },
        WireEvent::Leave => BrokerEvent::Leave,
        WireEvent::Drop { paths, x, y } => BrokerEvent::Drop { paths, x, y },
        WireEvent::Error { message } => BrokerEvent::Error { message },
    }
}

struct ShellSession {
    token: HANDLE,
    user_sid: String,
}

impl ShellSession {
    fn resolve() -> Result<Self, String> {
        let process_id = shell_process_id().ok_or_else(|| "找不到资源管理器进程".to_string())?;
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }
            .map_err(|error| format!("打开资源管理器进程失败：{error}"))?;
        let mut query_token = HANDLE::default();
        let opened =
            unsafe { OpenProcessToken(process, TOKEN_QUERY | TOKEN_DUPLICATE, &mut query_token) };
        unsafe {
            let _ = CloseHandle(process);
        }
        opened.map_err(|error| format!("读取资源管理器令牌失败：{error}"))?;

        let user_sid = crate::elevation::user_sid_string(query_token)
            .ok_or_else(|| "读取资源管理器用户 SID 失败".to_string())?;
        let mut primary = HANDLE::default();
        let duplicated = unsafe {
            DuplicateTokenEx(
                query_token,
                TOKEN_ALL_ACCESS,
                None,
                windows::Win32::Security::SecurityImpersonation,
                TokenPrimary,
                &mut primary,
            )
        };
        unsafe {
            let _ = CloseHandle(query_token);
        }
        duplicated.map_err(|error| format!("复制资源管理器令牌失败：{error}"))?;
        Ok(Self {
            token: primary,
            user_sid,
        })
    }
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.token);
        }
    }
}

fn shell_process_id() -> Option<u32> {
    let shell = unsafe { GetShellWindow() };
    if !shell.0.is_null() {
        let mut process_id = 0_u32;
        unsafe {
            GetWindowThreadProcessId(shell, Some(&mut process_id as *mut u32));
        }
        if process_id != 0 {
            return Some(process_id);
        }
    }
    let current_session = current_session_id();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = None;
    unsafe {
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                if entry_name(&entry).eq_ignore_ascii_case("explorer.exe") {
                    let mut session = 0_u32;
                    if ProcessIdToSessionId(entry.th32ProcessID, &mut session).is_ok()
                        && Some(session) == current_session
                    {
                        found = Some(entry.th32ProcessID);
                        break;
                    }
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

fn entry_name(entry: &PROCESSENTRY32W) -> String {
    let length = entry
        .szExeFile
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(entry.szExeFile.len());
    String::from_utf16_lossy(&entry.szExeFile[..length])
}

fn current_session_id() -> Option<u32> {
    let mut session = 0_u32;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }
        .ok()
        .map(|_| session)
}

fn spawn_broker(
    shell: &ShellSession,
    exe: &Path,
    pipe_name: &str,
    token: &str,
) -> Result<u32, String> {
    enable_privilege(SE_IMPERSONATE_NAME);
    let exe_wide = super::wide(&exe.to_string_lossy());
    let command = format!(
        "\"{}\" {} {} {}",
        exe.display(),
        super::BROKER_ARG,
        pipe_name,
        token
    );
    let mut last_error = String::new();
    for flags in [LOGON_WITH_PROFILE, CREATE_PROCESS_LOGON_FLAGS(0)] {
        let mut command_wide = super::wide(&command);
        let mut desktop = super::wide("winsta0\\default");
        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut information = PROCESS_INFORMATION::default();
        let result = unsafe {
            CreateProcessWithTokenW(
                shell.token,
                flags,
                PCWSTR(exe_wide.as_ptr()),
                Some(PWSTR(command_wide.as_mut_ptr())),
                PROCESS_CREATION_FLAGS(0),
                None,
                PCWSTR::null(),
                &startup,
                &mut information,
            )
        };
        match result {
            Ok(()) => {
                let process_id = information.dwProcessId;
                unsafe {
                    let _ = CloseHandle(information.hThread);
                    let _ = CloseHandle(information.hProcess);
                }
                return Ok(process_id);
            }
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(format!("以用户权限启动文件拖放代理失败：{last_error}"))
}

fn enable_privilege(name: PCWSTR) {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .is_err()
        {
            return;
        }
        let mut luid = windows::Win32::Foundation::LUID::default();
        if LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid).is_ok() {
            let privileges = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            let _ = AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None);
        }
        let _ = CloseHandle(token);
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0_u8; bytes];
    rand::thread_rng().fill_bytes(&mut buffer);
    let mut text = String::with_capacity(buffer.len() * 2);
    for byte in buffer {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

struct PipeSecurity {
    descriptor: SECURITY_DESCRIPTOR,
    acl: *mut ACL,
}

impl PipeSecurity {
    fn new(user_sid: &str) -> Result<Self, String> {
        let system = string_sid(SYSTEM_SID)?;
        let user = string_sid(user_sid)?;
        let entries = [
            explicit_access(system, GENERIC_READ.0 | GENERIC_WRITE.0),
            explicit_access(user, GENERIC_READ.0 | GENERIC_WRITE.0),
        ];
        let mut acl: *mut ACL = null_mut();
        let result = unsafe { SetEntriesInAclW(Some(&entries), None, &mut acl) };
        unsafe {
            let _ = LocalFree(Some(HLOCAL(system.0)));
            let _ = LocalFree(Some(HLOCAL(user.0)));
        }
        if result.0 != 0 {
            return Err(format!(
                "创建文件拖放代理管道 ACL 失败：{}",
                std::io::Error::from_raw_os_error(result.0 as i32)
            ));
        }
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let descriptor_ptr =
            PSECURITY_DESCRIPTOR((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast());
        unsafe {
            InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION)
                .map_err(|error| format!("初始化安全描述符失败：{error}"))?;
            SetSecurityDescriptorDacl(descriptor_ptr, true, Some(acl), false)
                .map_err(|error| format!("设置管道 DACL 失败：{error}"))?;
        }
        Ok(Self { descriptor, acl })
    }

    fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: (&mut self.descriptor as *mut SECURITY_DESCRIPTOR).cast(),
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.acl.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.acl.cast())));
            }
        }
    }
}

fn string_sid(text: &str) -> Result<PSID, String> {
    let text = super::wide(text);
    let mut sid = PSID(null_mut());
    unsafe {
        ConvertStringSidToSidW(PCWSTR(text.as_ptr()), &mut sid)
            .map_err(|error| format!("解析 SID 失败：{error}"))?;
    }
    Ok(sid)
}

fn explicit_access(sid: PSID, access: u32) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: ACE_FLAGS(0),
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: Default::default(),
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: PWSTR(sid.0.cast()),
        },
    }
}
