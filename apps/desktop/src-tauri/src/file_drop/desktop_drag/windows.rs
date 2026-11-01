use std::ffi::c_void;
use std::sync::OnceLock;

use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetClassNameW, GetMessageW, IsWindow, IsWindowVisible, KillTimer, SetTimer,
    TranslateMessage, EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY, MSG, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_TIMER,
};

use super::{DragSignal, SignalEmitter};

const POLL_INTERVAL_MS: u32 = 120;
const DRAG_IMAGE_CLASS: &str = "SysDragImage";
const KEY_DOWN: u16 = 0x8000;

static DRAG_IMAGES: OnceLock<Mutex<Vec<isize>>> = OnceLock::new();

fn drag_images() -> &'static Mutex<Vec<isize>> {
    DRAG_IMAGES.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn spawn(sender: UnboundedSender<DragSignal>) {
    let _ = std::thread::Builder::new()
        .name("nc-desktop-drag".into())
        .spawn(move || run(sender));
}

fn run(sender: UnboundedSender<DragSignal>) {
    unsafe {
        let hook = SetWinEventHook(
            EVENT_OBJECT_CREATE,
            EVENT_OBJECT_DESTROY,
            None,
            Some(on_win_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        if hook.is_invalid() {
            tracing::warn!("注册拖动监听 WinEvent 钩子失败");
            return;
        }
        let timer = SetTimer(None, 0, POLL_INTERVAL_MS, None);
        if timer == 0 {
            tracing::warn!("启动拖动检测定时器失败");
            let _ = UnhookWinEvent(hook);
            return;
        }
        let mut emitter = SignalEmitter::new(sender);
        let mut probe = DragProbe::default();
        let mut message = MSG::default();
        loop {
            let received = GetMessageW(&mut message, None, 0, 0);
            if received.0 <= 0 {
                break;
            }
            if message.message == WM_TIMER {
                if !emitter.update(probe.poll()) {
                    break;
                }
                continue;
            }
            let _ = TranslateMessage(&message);
            let _ = DispatchMessageW(&message);
        }
        let _ = KillTimer(None, timer);
        let _ = UnhookWinEvent(hook);
    }
}

unsafe extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd.0.is_null() {
        return;
    }
    let handle = hwnd.0 as isize;
    if event == EVENT_OBJECT_DESTROY {
        drag_images()
            .lock()
            .retain(|candidate| *candidate != handle);
        return;
    }
    if event != EVENT_OBJECT_CREATE || id_object != 0 || id_child != 0 {
        return;
    }
    let mut buffer = [0u16; 64];
    let length = GetClassNameW(hwnd, &mut buffer);
    if length <= 0 {
        return;
    }
    let class = String::from_utf16_lossy(&buffer[..length as usize]);
    if !class.eq_ignore_ascii_case(DRAG_IMAGE_CLASS) {
        return;
    }
    let mut images = drag_images().lock();
    if !images.contains(&handle) {
        images.push(handle);
    }
}

#[derive(Default)]
struct DragProbe {
    active: bool,
}

impl DragProbe {
    fn poll(&mut self) -> bool {
        let button_down = left_button_down();
        if !button_down {
            drag_images().lock().clear();
        }
        self.active = next_active(
            self.active,
            button_down,
            button_down && any_visible_drag_image(),
        );
        self.active
    }
}

fn next_active(active: bool, button_down: bool, visible: bool) -> bool {
    button_down && (active || visible)
}

fn left_button_down() -> bool {
    (unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) }) as u16 & KEY_DOWN != 0
}

fn any_visible_drag_image() -> bool {
    let mut images = drag_images().lock();
    images.retain(|handle| {
        let hwnd = HWND(*handle as *mut c_void);
        unsafe { IsWindow(Some(hwnd)).as_bool() }
    });
    images.iter().any(|handle| {
        let hwnd = HWND(*handle as *mut c_void);
        unsafe { IsWindowVisible(hwnd).as_bool() }
    })
}
