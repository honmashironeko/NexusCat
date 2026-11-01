use super::proto::{self, Command, Event, Hello, Welcome, PROTOCOL_VERSION};
use std::collections::VecDeque;
use std::fs::File;
use std::os::windows::io::FromRawHandle;
use std::ptr;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use windows::core::{implement, Ref, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, POINTL, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE, OPEN_EXISTING,
};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, OleInitialize, OleUninitialize, RegisterDragDrop,
    ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostQuitMessage,
    PostThreadMessageW, RegisterClassW, SetLayeredWindowAttributes, SetWindowPos, ShowWindow,
    TranslateMessage, HTCLIENT, HWND_TOPMOST, LWA_ALPHA, MSG, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SW_HIDE, WM_APP, WM_DESTROY, WM_ERASEBKGND, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const WM_APP_COMMAND: u32 = WM_APP + 1;
const CONNECT_ATTEMPTS: u32 = 50;
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(100);
const CLASS_NAME: &str = "NexusCatFileDropSurface";

pub fn run(pipe_name: &str, token: &str) -> i32 {
    match run_inner(pipe_name, token) {
        Ok(()) => 0,
        Err(error) => {
            tracing::error!(%error, "文件拖放代理退出");
            1
        }
    }
}

fn run_inner(pipe_name: &str, token: &str) -> Result<(), String> {
    let mut channel = connect(pipe_name)?;
    proto::write_frame(
        &mut channel,
        &Hello {
            token: token.to_string(),
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .map_err(io_message)?;
    let welcome: Welcome = proto::read_frame(&mut channel).map_err(io_message)?;
    if welcome.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "文件拖放代理协议版本不一致：{} != {}",
            welcome.protocol_version, PROTOCOL_VERSION
        ));
    }
    tracing::info!("文件拖放代理已连接桌面应用");

    let reader = channel.try_clone().map_err(io_message)?;
    let (events_tx, events_rx) = mpsc::channel::<Event>();
    thread::Builder::new()
        .name("nc-drop-broker-events".into())
        .spawn(move || event_writer(channel, events_rx))
        .map_err(io_message)?;

    unsafe { OleInitialize(None) }.map_err(|error| format!("初始化 OLE 失败：{error}"))?;
    let hwnd = match create_surface() {
        Ok(hwnd) => hwnd,
        Err(error) => {
            let _ = events_tx.send(Event::Error {
                message: error.clone(),
            });
            unsafe { OleUninitialize() };
            return Err(error);
        }
    };
    let target: IDropTarget = BrokerDropTarget {
        events: events_tx.clone(),
        hwnd,
        accepted: std::cell::Cell::new(false),
        effect: std::cell::Cell::new(DROPEFFECT_NONE),
    }
    .into();
    if let Err(error) = unsafe { RegisterDragDrop(hwnd, &target) } {
        let _ = events_tx.send(Event::Error {
            message: format!("注册拖放目标失败：{error}"),
        });
        unsafe {
            let _ = DestroyWindow(hwnd);
            OleUninitialize();
        }
        return Err(format!("注册拖放目标失败：{error}"));
    }

    let commands: Arc<parking_lot::Mutex<VecDeque<Command>>> =
        Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let ui_thread = unsafe { GetCurrentThreadId() };
    let command_queue = Arc::clone(&commands);
    thread::Builder::new()
        .name("nc-drop-broker-commands".into())
        .spawn(move || command_reader(reader, command_queue, ui_thread))
        .map_err(io_message)?;

    message_loop(hwnd, &commands);

    unsafe {
        let _ = RevokeDragDrop(hwnd);
        let _ = DestroyWindow(hwnd);
        OleUninitialize();
    }
    tracing::info!("文件拖放代理已停止");
    Ok(())
}

fn connect(pipe_name: &str) -> Result<File, String> {
    let name = super::wide(pipe_name);
    let mut last_error = String::new();
    for _ in 0..CONNECT_ATTEMPTS {
        match unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                (windows::Win32::Foundation::GENERIC_READ
                    | windows::Win32::Foundation::GENERIC_WRITE)
                    .0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        } {
            Ok(handle) => {
                return Ok(unsafe { File::from_raw_handle(handle.0 as _) });
            }
            Err(error) => last_error = error.to_string(),
        }
        thread::sleep(CONNECT_RETRY_DELAY);
    }
    Err(format!("连接文件拖放代理管道失败：{last_error}"))
}

fn event_writer(mut channel: File, events: mpsc::Receiver<Event>) {
    while let Ok(event) = events.recv() {
        if let Err(error) = proto::write_frame(&mut channel, &event) {
            tracing::warn!(%error, "发送文件拖放代理事件失败");
            break;
        }
    }
}

fn command_reader(
    mut channel: File,
    commands: Arc<parking_lot::Mutex<VecDeque<Command>>>,
    ui_thread: u32,
) {
    loop {
        let command = match proto::read_frame::<_, Command>(&mut channel) {
            Ok(command) => command,
            Err(_) => Command::Shutdown,
        };
        let shutdown = matches!(command, Command::Shutdown);
        commands.lock().push_back(command);
        if unsafe { PostThreadMessageW(ui_thread, WM_APP_COMMAND, WPARAM(0), LPARAM(0)) }.is_err() {
            break;
        }
        if shutdown {
            break;
        }
    }
}

fn message_loop(hwnd: HWND, commands: &parking_lot::Mutex<VecDeque<Command>>) {
    let mut message = MSG::default();
    loop {
        let received = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if received.0 <= 0 {
            break;
        }
        if message.message == WM_APP_COMMAND {
            if !drain_commands(hwnd, commands) {
                break;
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn drain_commands(hwnd: HWND, commands: &parking_lot::Mutex<VecDeque<Command>>) -> bool {
    loop {
        let Some(command) = commands.lock().pop_front() else {
            return true;
        };
        match command {
            Command::Show {
                x,
                y,
                width,
                height,
            } => {
                let result = unsafe {
                    SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        x,
                        y,
                        width,
                        height,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    )
                };
                if let Err(error) = result {
                    tracing::warn!(%error, "定位文件拖放代理窗口失败");
                }
            }
            Command::Hide => {
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
            Command::Shutdown => return false,
        }
    }
}

fn create_surface() -> Result<HWND, String> {
    let class_name = super::wide(CLASS_NAME);
    let title = super::wide("NexusCat Drop Surface");
    let module =
        unsafe { GetModuleHandleW(None) }.map_err(|error| format!("读取模块句柄失败：{error}"))?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: module.into(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(format!(
            "注册拖放代理窗口类失败：{}",
            std::io::Error::last_os_error()
        ));
    }
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(module.into()),
            None,
        )
    }
    .map_err(|error| format!("创建拖放代理窗口失败：{error}"))?;
    unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA) }
        .map_err(|error| format!("设置拖放代理窗口透明度失败：{error}"))?;
    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    Ok(hwnd)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[implement(IDropTarget)]
struct BrokerDropTarget {
    events: Sender<Event>,
    hwnd: HWND,
    accepted: std::cell::Cell<bool>,
    effect: std::cell::Cell<DROPEFFECT>,
}

impl BrokerDropTarget {
    fn point(&self, point: &POINTL) -> (i32, i32) {
        let mut local = POINT {
            x: point.x,
            y: point.y,
        };
        let _ = unsafe { ScreenToClient(self.hwnd, &mut local) };
        (local.x, local.y)
    }
}

impl IDropTarget_Impl for BrokerDropTarget_Impl {
    fn DragEnter(
        &self,
        data_object: Ref<'_, IDataObject>,
        _key_state: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let Some(data_object) = data_object.as_ref() else {
            unsafe { *effect = DROPEFFECT_NONE };
            return Ok(());
        };
        let paths = unsafe { read_paths(data_object) }.unwrap_or_default();
        if paths.is_empty() {
            unsafe { *effect = DROPEFFECT_NONE };
            return Ok(());
        }
        self.accepted.set(true);
        self.effect.set(DROPEFFECT_COPY);
        let (x, y) = self.point(point);
        let _ = self.events.send(Event::Enter {
            count: paths.len().min(proto::MAX_DROP_PATHS) as u32,
            x,
            y,
        });
        unsafe { *effect = DROPEFFECT_COPY };
        Ok(())
    }

    fn DragOver(
        &self,
        _key_state: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        if self.accepted.get() {
            let (x, y) = self.point(point);
            let _ = self.events.send(Event::Over { x, y });
        }
        unsafe { *effect = self.effect.get() };
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if self.accepted.replace(false) {
            let _ = self.events.send(Event::Leave);
        }
        Ok(())
    }

    fn Drop(
        &self,
        data_object: Ref<'_, IDataObject>,
        _key_state: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let accepted = self.accepted.replace(false);
        if accepted {
            let (x, y) = self.point(point);
            let paths = data_object
                .as_ref()
                .and_then(|data_object| unsafe { read_paths(data_object) })
                .unwrap_or_default();
            if !paths.is_empty() {
                let _ = self.events.send(Event::Drop { paths, x, y });
            }
        }
        unsafe {
            *effect = if accepted {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            }
        };
        Ok(())
    }
}

unsafe fn read_paths(data_object: &IDataObject) -> Option<Vec<String>> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let mut medium = unsafe { data_object.GetData(&format) }.ok()?;
    if medium.tymed != TYMED_HGLOBAL.0 as u32 || medium.u.hGlobal.0.is_null() {
        unsafe { ReleaseStgMedium(&mut medium) };
        return None;
    }
    let drop = HDROP(medium.u.hGlobal.0);
    let count = unsafe { DragQueryFileW(drop, 0xFFFF_FFFF, None) };
    let mut paths = Vec::new();
    let mut total_bytes = 0_usize;
    for index in 0..count.min(proto::MAX_DROP_PATHS as u32) {
        let length = unsafe { DragQueryFileW(drop, index, None) } as usize;
        if length == 0 {
            continue;
        }
        let mut buffer = vec![0_u16; length + 1];
        let written = unsafe { DragQueryFileW(drop, index, Some(&mut buffer)) } as usize;
        if written == 0 {
            continue;
        }
        let path = String::from_utf16_lossy(&buffer[..written]);
        if total_bytes + path.len() > proto::MAX_DROP_PATH_BYTES {
            break;
        }
        total_bytes += path.len();
        paths.push(path);
    }
    unsafe { ReleaseStgMedium(&mut medium) };
    Some(paths)
}

fn io_message(error: std::io::Error) -> String {
    error.to_string()
}
