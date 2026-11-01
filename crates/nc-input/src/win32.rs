use std::collections::HashMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{SendTimeoutError, Sender};
use nc_core::{InputEvent, KeyState, Modifiers, MouseButton, MouseButtonState, NexusError, Result};
use nc_platform::RectI;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY, VK_CAPITAL, VK_NUMLOCK,
};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT,
    RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEMOUSE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetMessageW, PeekMessageW,
    PostThreadMessageW, RegisterClassW, SetWindowsHookExW, UnhookWindowsHookEx, HWND_MESSAGE,
    KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT,
    PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_INPUT,
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW, XBUTTON1, XBUTTON2,
};

use crate::{unix_timestamp_us, CapturedInput, InputCapture, InputInjector};

const HOOK_JOIN_TIMEOUT: Duration = Duration::from_secs(2);
const SINK_SEND_TIMEOUT: Duration = Duration::from_millis(10);
const VIRTUAL_BOUNDS_TTL: Duration = Duration::from_secs(2);
const RELATIVE_MODE_MESSAGE: u32 = WM_APP + 1;
const LOCAL_INPUT_RESYNC_MESSAGE: u32 = WM_APP + 2;
const INJECTION_GUARD_WINDOW: Duration = Duration::from_millis(2);
const RAW_INPUT_USAGE_PAGE_MOUSE: u16 = 1;
const RAW_INPUT_USAGE_MOUSE: u16 = 2;

static INJECTION_GUARD_UNTIL_US: AtomicU64 = AtomicU64::new(0);
static MOVE_LATCH: AtomicU64 = AtomicU64::new(0);
static INJECTION_GUARD_EPOCH: OnceLock<Instant> = OnceLock::new();
static RAW_INPUT_CLASS_REGISTRATION: Once = Once::new();
static RAW_INPUT_CLASS_NAME: OnceLock<Vec<u16>> = OnceLock::new();

pub(crate) struct WindowsInputCapture {
    state: Mutex<CaptureState>,
    suppressed: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    relative_mode: Arc<AtomicBool>,
    absolute_pointer: Arc<AtomicBool>,
}

#[derive(Default)]
struct CaptureState {
    worker: Option<CaptureWorker>,
}

struct CaptureWorker {
    thread_id: u32,
    join: JoinHandle<()>,
}

impl WindowsInputCapture {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(CaptureState::default()),
            suppressed: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            relative_mode: Arc::new(AtomicBool::new(false)),
            absolute_pointer: Arc::new(AtomicBool::new(false)),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, CaptureState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl InputCapture for WindowsInputCapture {
    fn start(&mut self, sink: Sender<CapturedInput>) -> Result<()> {
        let mut state = self.lock_state();

        if self.running.load(Ordering::Acquire) {
            return Err(NexusError::Business("输入捕获已经在运行".to_string()));
        }
        if let Some(worker) = state.worker.take() {
            join_with_timeout(worker.join, HOOK_JOIN_TIMEOUT);
        }

        self.running.store(true, Ordering::Release);
        let (ready_tx, ready_rx) =
            crossbeam_channel::bounded::<std::result::Result<u32, String>>(1);
        let suppressed = Arc::clone(&self.suppressed);
        let running = Arc::clone(&self.running);
        let relative_mode = Arc::clone(&self.relative_mode);
        let absolute_pointer = Arc::clone(&self.absolute_pointer);

        let join = match thread::Builder::new()
            .name("nc-input-hooks".to_string())
            .spawn(move || {
                capture_thread(
                    sink,
                    suppressed,
                    running,
                    relative_mode,
                    absolute_pointer,
                    ready_tx,
                )
            }) {
            Ok(join) => join,
            Err(error) => {
                self.running.store(false, Ordering::Release);
                return Err(NexusError::platform(format!(
                    "启动输入捕获线程失败：{error}"
                )));
            }
        };

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => {
                state.worker = Some(CaptureWorker { thread_id, join });
                Ok(())
            }
            Ok(Err(message)) => {
                join_with_timeout(join, HOOK_JOIN_TIMEOUT);
                self.running.store(false, Ordering::Release);
                Err(NexusError::platform(message))
            }
            Err(error) => {
                join_with_timeout(join, HOOK_JOIN_TIMEOUT);
                self.running.store(false, Ordering::Release);
                Err(NexusError::platform(format!(
                    "输入捕获线程启动时退出：{error}"
                )))
            }
        }
    }

    fn stop(&mut self) {
        let mut state = self.lock_state();
        self.running.store(false, Ordering::Release);
        self.suppressed.store(false, Ordering::Release);
        self.relative_mode.store(false, Ordering::Release);
        self.absolute_pointer.store(false, Ordering::Release);

        if let Some(worker) = state.worker.take() {
            unsafe {
                if let Err(error) =
                    PostThreadMessageW(worker.thread_id, WM_QUIT, WPARAM(0), LPARAM(0))
                {
                    tracing::warn!(%error, "向输入捕获线程发送 WM_QUIT 失败");
                }
            }
            join_with_timeout(worker.join, HOOK_JOIN_TIMEOUT);
        }
    }

    fn set_suppressed(&self, suppressed: bool) {
        let previous = self.suppressed.swap(suppressed, Ordering::AcqRel);
        if !suppressed || previous {
            return;
        }
        let state = self.lock_state();
        let Some(worker) = state.worker.as_ref() else {
            return;
        };
        unsafe {
            if let Err(error) = PostThreadMessageW(
                worker.thread_id,
                LOCAL_INPUT_RESYNC_MESSAGE,
                WPARAM(0),
                LPARAM(0),
            ) {
                tracing::warn!(%error, "向输入捕获线程发送本机按键归位消息失败");
            }
        }
    }

    fn is_suppressed(&self) -> bool {
        self.suppressed.load(Ordering::Acquire)
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn set_relative_mode(&self, enabled: bool) {
        self.relative_mode.store(enabled, Ordering::Release);
        let state = self.lock_state();
        let Some(worker) = state.worker.as_ref() else {
            return;
        };
        unsafe {
            if let Err(error) = PostThreadMessageW(
                worker.thread_id,
                RELATIVE_MODE_MESSAGE,
                WPARAM(0),
                LPARAM(0),
            ) {
                tracing::warn!(%error, "向输入捕获线程发送相对模式切换消息失败");
            }
        }
    }

    fn set_absolute_pointer(&self, enabled: bool) {
        self.absolute_pointer.store(enabled, Ordering::Release);
    }

    fn latch_mouse_moves(&self, direction: nc_core::Direction, duration_ms: u64) {
        let code = latch_code(direction);
        let until = monotonic_micros().saturating_add(duration_ms.saturating_mul(1000));
        MOVE_LATCH.store((until << 3) | code, Ordering::Release);
    }
}

fn latch_code(direction: nc_core::Direction) -> u64 {
    match direction {
        nc_core::Direction::Left => 1,
        nc_core::Direction::Right => 2,
        nc_core::Direction::Up => 3,
        nc_core::Direction::Down => 4,
    }
}

fn latched_move_direction() -> Option<nc_core::Direction> {
    let packed = MOVE_LATCH.load(Ordering::Acquire);
    if packed == 0 {
        return None;
    }
    if monotonic_micros() >= packed >> 3 {
        return None;
    }
    match packed & 0b111 {
        1 => Some(nc_core::Direction::Left),
        2 => Some(nc_core::Direction::Right),
        3 => Some(nc_core::Direction::Up),
        4 => Some(nc_core::Direction::Down),
        _ => None,
    }
}

fn latched_move_swallowed(runtime: &HookRuntime, info: &MSLLHOOKSTRUCT) -> bool {
    let Some(direction) = latched_move_direction() else {
        return false;
    };
    let (last_x, last_y) = nc_platform::cursor_position()
        .ok()
        .or(runtime.last_mouse)
        .unwrap_or((info.pt.x, info.pt.y));
    move_matches_latch(
        direction,
        info.pt.x.saturating_sub(last_x),
        info.pt.y.saturating_sub(last_y),
    )
}

fn move_matches_latch(direction: nc_core::Direction, delta_x: i32, delta_y: i32) -> bool {
    if delta_x == 0 && delta_y == 0 {
        return false;
    }
    let moving = if delta_x.abs() >= delta_y.abs() {
        if delta_x < 0 {
            nc_core::Direction::Left
        } else {
            nc_core::Direction::Right
        }
    } else if delta_y < 0 {
        nc_core::Direction::Up
    } else {
        nc_core::Direction::Down
    };
    moving == direction
}

impl Drop for WindowsInputCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn join_with_timeout(join: JoinHandle<()>, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !join.is_finished() {
        if Instant::now() >= deadline {
            tracing::warn!("等待输入捕获线程退出超时，线程将被分离");
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    if join.join().is_err() {
        tracing::warn!("输入捕获线程异常结束");
    }
}

struct RunningGuard(Arc<AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct HookRuntime {
    sink: Sender<CapturedInput>,
    suppressed: Arc<AtomicBool>,
    absolute_pointer: Arc<AtomicBool>,
    modifiers: ModifierState,
    last_mouse: Option<(i32, i32)>,
    relative_mode: bool,
    raw_window: Option<HWND>,
    local_keys_down: Vec<u32>,
    local_buttons_down: Vec<MouseButton>,
}

impl Drop for HookRuntime {
    fn drop(&mut self) {
        self.disable_raw_input();
    }
}

thread_local! {
    static HOOK_RUNTIME: std::cell::RefCell<Option<HookRuntime>> =
        const { std::cell::RefCell::new(None) };
}

fn capture_thread(
    sink: Sender<CapturedInput>,
    suppressed: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    relative_mode: Arc<AtomicBool>,
    absolute_pointer: Arc<AtomicBool>,
    ready: Sender<std::result::Result<u32, String>>,
) {
    let _running_guard = RunningGuard(running);
    let thread_id = unsafe { GetCurrentThreadId() };

    let mut message = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }

    let caps_lock = (unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) } & 1) != 0;
    let num_lock = (unsafe { GetKeyState(i32::from(VK_NUMLOCK.0)) } & 1) != 0;
    let runtime = HookRuntime {
        sink,
        suppressed,
        absolute_pointer,
        modifiers: ModifierState::new(caps_lock, num_lock),
        last_mouse: nc_platform::cursor_position().ok(),
        relative_mode: false,
        raw_window: None,
        local_keys_down: Vec::new(),
        local_buttons_down: Vec::new(),
    };
    HOOK_RUNTIME.with(|slot| {
        *slot.borrow_mut() = Some(runtime);
    });

    let module = unsafe { GetModuleHandleW(None) }
        .ok()
        .map(|handle| HINSTANCE(handle.0));
    let keyboard_hook =
        match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), module, 0) } {
            Ok(hook) => hook,
            Err(error) => {
                clear_runtime();
                let _ = ready.send(Err(format!("安装 WH_KEYBOARD_LL 失败：{error}")));
                return;
            }
        };
    let mouse_hook =
        match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), module, 0) } {
            Ok(hook) => hook,
            Err(error) => {
                let _ = unsafe { UnhookWindowsHookEx(keyboard_hook) };
                clear_runtime();
                let _ = ready.send(Err(format!("安装 WH_MOUSE_LL 失败：{error}")));
                return;
            }
        };

    if ready.send(Ok(thread_id)).is_err() {
        let _ = unsafe { UnhookWindowsHookEx(mouse_hook) };
        let _ = unsafe { UnhookWindowsHookEx(keyboard_hook) };
        clear_runtime();
        return;
    }

    if relative_mode.load(Ordering::Acquire) {
        apply_relative_mode(true);
    }

    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 == 0 {
            break;
        }
        if result.0 == -1 {
            tracing::error!("输入捕获线程 GetMessageW 失败");
            break;
        }

        match message.message {
            RELATIVE_MODE_MESSAGE => {
                apply_relative_mode(relative_mode.load(Ordering::Acquire));
            }
            LOCAL_INPUT_RESYNC_MESSAGE => {
                release_local_held_inputs();
            }
            WM_INPUT => {
                handle_raw_input(message.lParam);
                let _ = unsafe {
                    DefWindowProcW(
                        message.hwnd,
                        message.message,
                        message.wParam,
                        message.lParam,
                    )
                };
            }
            _ => {}
        }
    }

    let _ = unsafe { UnhookWindowsHookEx(mouse_hook) };
    let _ = unsafe { UnhookWindowsHookEx(keyboard_hook) };
    clear_runtime();
}

fn clear_runtime() {
    HOOK_RUNTIME.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            *slot = None;
        }
    });
}

fn apply_relative_mode(enabled: bool) {
    HOOK_RUNTIME.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            if let Some(runtime) = slot.as_mut() {
                if enabled {
                    runtime.enable_raw_input();
                } else {
                    runtime.disable_raw_input();
                }
            }
        }
    });
}

fn handle_raw_input(lparam: LPARAM) {
    HOOK_RUNTIME.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            if let Some(runtime) = slot.as_mut() {
                let input = HRAWINPUT(lparam.0 as *mut core::ffi::c_void);
                if let Some(captured) = process_raw_mouse(runtime, input) {
                    let _ = send_captured(&runtime.sink, &runtime.suppressed, captured);
                }
            }
        }
    });
}

fn process_raw_mouse(runtime: &mut HookRuntime, input: HRAWINPUT) -> Option<CapturedInput> {
    if !runtime.relative_mode || injection_guard_active() {
        return None;
    }

    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let copied = unsafe {
        GetRawInputData(
            input,
            RID_INPUT,
            Some(&mut raw as *mut RAWINPUT as *mut core::ffi::c_void),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if copied == 0 || copied == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE.0 {
        return None;
    }

    let mouse = unsafe { raw.data.mouse };
    if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 {
        return None;
    }

    let delta_x = mouse.lLastX;
    let delta_y = mouse.lLastY;
    if delta_x == 0 && delta_y == 0 {
        return None;
    }

    let (x, y) =
        nc_platform::cursor_position().unwrap_or_else(|_| runtime.last_mouse.unwrap_or((0, 0)));
    runtime.last_mouse = Some((x, y));

    Some(CapturedInput {
        event: InputEvent::MouseMove {
            x,
            y,
            delta_x,
            delta_y,
            modifiers: runtime.modifiers.current(),
        },
        timestamp_us: unix_timestamp_us(),
        injected: false,
        raw_relative: true,
        local_only: false,
    })
}

impl HookRuntime {
    fn enable_raw_input(&mut self) {
        if self.relative_mode {
            return;
        }
        let hwnd = match create_raw_input_window() {
            Ok(hwnd) => hwnd,
            Err(error) => {
                tracing::warn!(%error, "创建 Raw Input 消息窗口失败，相对鼠标采集不可用");

                return;
            }
        };

        let device = RAWINPUTDEVICE {
            usUsagePage: RAW_INPUT_USAGE_PAGE_MOUSE,
            usUsage: RAW_INPUT_USAGE_MOUSE,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        match unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) } {
            Ok(()) => {
                self.raw_window = Some(hwnd);
                self.relative_mode = true;

                tracing::debug!("Raw Input 相对鼠标采集已开启");
            }
            Err(error) => {
                tracing::warn!(%error, "注册 Raw Input 鼠标设备失败，相对鼠标采集不可用");

                let _ = unsafe { DestroyWindow(hwnd) };
            }
        }
    }

    fn disable_raw_input(&mut self) {
        if let Some(hwnd) = self.raw_window.take() {
            let device = RAWINPUTDEVICE {
                usUsagePage: RAW_INPUT_USAGE_PAGE_MOUSE,
                usUsage: RAW_INPUT_USAGE_MOUSE,
                dwFlags: RIDEV_REMOVE,
                hwndTarget: HWND::default(),
            };
            if let Err(error) =
                unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) }
            {
                tracing::warn!(%error, "注销 Raw Input 鼠标设备失败");
            }
            if let Err(error) = unsafe { DestroyWindow(hwnd) } {
                tracing::warn!(%error, "销毁 Raw Input 消息窗口失败");
            }
        }
        self.relative_mode = false;
    }
}

fn raw_input_class_name() -> PCWSTR {
    let name = RAW_INPUT_CLASS_NAME.get_or_init(|| "NexusCatRawInput\0".encode_utf16().collect());
    PCWSTR(name.as_ptr())
}

fn create_raw_input_window() -> windows::core::Result<HWND> {
    let module = unsafe { GetModuleHandleW(None) }?;
    let instance = HINSTANCE(module.0);
    RAW_INPUT_CLASS_REGISTRATION.call_once(|| {
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(raw_input_window_proc),
            hInstance: instance,
            lpszClassName: raw_input_class_name(),
            ..Default::default()
        };
        let atom = unsafe { RegisterClassW(&window_class) };
        if atom == 0 {
            let error = unsafe { GetLastError() };
            tracing::warn!("注册 Raw Input 窗口类失败：GetLastError={}", error.0);
        }
    });

    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            raw_input_class_name(),
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
    }
}

unsafe extern "system" fn raw_input_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn monotonic_micros() -> u64 {
    let elapsed = INJECTION_GUARD_EPOCH.get_or_init(Instant::now).elapsed();
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

fn arm_injection_guard() {
    let until = monotonic_micros().saturating_add(INJECTION_GUARD_WINDOW.as_micros() as u64);
    INJECTION_GUARD_UNTIL_US.store(until, Ordering::Release);
}

fn injection_guard_active() -> bool {
    monotonic_micros() < INJECTION_GUARD_UNTIL_US.load(Ordering::Acquire)
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let mut swallow = false;
    HOOK_RUNTIME.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            if let Some(runtime) = slot.as_mut() {
                if let Some(captured) =
                    process_keyboard(runtime, wparam.0 as u32, lparam.0 as *const _)
                {
                    swallow = dispatch_captured(runtime, captured);
                }
            }
        }
    });

    if swallow {
        LRESULT(1)
    } else {
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    if wparam.0 as u32 == WM_MOUSEMOVE {
        let swallow = HOOK_RUNTIME.with(|slot| {
            let Ok(slot) = slot.try_borrow() else {
                return false;
            };
            let Some(runtime) = slot.as_ref() else {
                return false;
            };
            if runtime.relative_mode {
                return true;
            }
            let info = lparam.0 as *const MSLLHOOKSTRUCT;
            !info.is_null() && latched_move_swallowed(runtime, unsafe { &*info })
        });
        if swallow {
            return LRESULT(1);
        }
    }

    let mut swallow = false;
    HOOK_RUNTIME.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            if let Some(runtime) = slot.as_mut() {
                if let Some(captured) =
                    process_mouse(runtime, wparam.0 as u32, lparam.0 as *const _)
                {
                    swallow = dispatch_captured(runtime, captured);
                }
            }
        }
    });

    if swallow {
        LRESULT(1)
    } else {
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

fn dispatch_captured(runtime: &mut HookRuntime, mut captured: CapturedInput) -> bool {
    let mut suppress = should_suppress_event(
        runtime.suppressed.load(Ordering::Acquire),
        runtime.absolute_pointer.load(Ordering::Acquire),
        captured.injected,
        &captured.event,
    );
    if suppress && is_release_of_local_input(runtime, &captured.event) {
        suppress = false;
        captured.local_only = true;
    }
    let local_press = LocalPress::from_event(&captured.event);
    let delivered = send_captured(&runtime.sink, &runtime.suppressed, captured);
    let swallow = suppress && delivered;
    if !swallow {
        if let Some(press) = local_press {
            runtime.note_local_press(press);
        }
    }
    swallow
}

#[derive(Clone, Copy)]
enum LocalPress {
    KeyDown(u32),
    KeyUp(u32),
    ButtonDown(MouseButton),
    ButtonUp(MouseButton),
}

impl LocalPress {
    fn from_event(event: &InputEvent) -> Option<Self> {
        match event {
            InputEvent::Key {
                state, key_code, ..
            } => Some(match state {
                KeyState::Down => Self::KeyDown(*key_code),
                KeyState::Up => Self::KeyUp(*key_code),
            }),
            InputEvent::MouseButton { state, button, .. } => Some(match state {
                MouseButtonState::Down => Self::ButtonDown(*button),
                MouseButtonState::Up => Self::ButtonUp(*button),
            }),
            _ => None,
        }
    }
}

fn is_release_of_local_input(runtime: &HookRuntime, event: &InputEvent) -> bool {
    match event {
        InputEvent::Key {
            state: KeyState::Up,
            key_code,
            ..
        } => runtime.local_keys_down.contains(key_code),
        InputEvent::MouseButton {
            state: MouseButtonState::Up,
            button,
            ..
        } => runtime.local_buttons_down.contains(button),
        _ => false,
    }
}

impl HookRuntime {
    fn note_local_press(&mut self, press: LocalPress) {
        match press {
            LocalPress::KeyDown(key_code) => {
                if !self.local_keys_down.contains(&key_code) {
                    self.local_keys_down.push(key_code);
                }
            }
            LocalPress::KeyUp(key_code) => {
                self.local_keys_down.retain(|held| *held != key_code);
            }
            LocalPress::ButtonDown(button) => {
                if !self.local_buttons_down.contains(&button) {
                    self.local_buttons_down.push(button);
                }
            }
            LocalPress::ButtonUp(button) => {
                self.local_buttons_down.retain(|held| *held != button);
            }
        }
    }
}

fn release_local_held_inputs() {
    let (keys, buttons) = HOOK_RUNTIME.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return (Vec::new(), Vec::new());
        };
        let Some(runtime) = slot.as_mut() else {
            return (Vec::new(), Vec::new());
        };
        (
            std::mem::take(&mut runtime.local_keys_down),
            std::mem::take(&mut runtime.local_buttons_down),
        )
    });
    if keys.is_empty() && buttons.is_empty() {
        return;
    }
    let mut inputs = Vec::with_capacity(keys.len() + buttons.len());
    for key_code in &keys {
        match keyboard_input(KeyState::Up, *key_code) {
            Ok(input) => inputs.push(input),
            Err(error) => tracing::debug!(%error, key_code, "补发本机按键抬起时缺少键位映射"),
        }
    }
    for button in &buttons {
        match mouse_button_flags(MouseButtonState::Up, *button) {
            Ok((flags, mouse_data)) => inputs.push(mouse_input(flags, 0, 0, mouse_data)),
            Err(error) => tracing::debug!(%error, ?button, "补发本机按钮抬起时缺少映射"),
        }
    }
    if let Err(error) = send_inputs(&inputs) {
        tracing::warn!(%error, "补发本机按键/按钮抬起失败，等待物理抬起");
        HOOK_RUNTIME.with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut() {
                if let Some(runtime) = slot.as_mut() {
                    for key_code in keys {
                        if !runtime.local_keys_down.contains(&key_code) {
                            runtime.local_keys_down.push(key_code);
                        }
                    }
                    for button in buttons {
                        if !runtime.local_buttons_down.contains(&button) {
                            runtime.local_buttons_down.push(button);
                        }
                    }
                }
            }
        });
    } else {
        tracing::debug!(events = inputs.len(), "已释放本机系统仍按下的输入");
    }
}

fn should_suppress_event(
    suppressed: bool,
    absolute_pointer: bool,
    injected: bool,
    event: &InputEvent,
) -> bool {
    if !suppressed || injected {
        return false;
    }
    let is_move = matches!(event, InputEvent::MouseMove { .. });
    !(absolute_pointer && is_move)
}

fn send_captured(
    sink: &Sender<CapturedInput>,
    suppressed: &AtomicBool,
    captured: CapturedInput,
) -> bool {
    match sink.send_timeout(captured, SINK_SEND_TIMEOUT) {
        Ok(()) => true,
        Err(SendTimeoutError::Timeout(_)) => {
            tracing::debug!("输入事件接收端繁忙，丢弃本条事件");
            false
        }
        Err(SendTimeoutError::Disconnected(_)) => {
            if suppressed.swap(false, Ordering::AcqRel) {
                tracing::warn!("输入事件接收端已断开，自动解除输入吞没");
            }
            false
        }
    }
}

fn process_keyboard(
    runtime: &mut HookRuntime,
    message: u32,
    info: *const KBDLLHOOKSTRUCT,
) -> Option<CapturedInput> {
    let state = match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => KeyState::Down,
        WM_KEYUP | WM_SYSKEYUP => KeyState::Up,
        _ => return None,
    };
    if info.is_null() {
        return None;
    }

    let info = unsafe { &*info };
    let extended = info.flags.contains(LLKHF_EXTENDED);
    let Some(key_code) = crate::keymap::hid_from_windows(info.vkCode, info.scanCode, extended)
    else {
        tracing::debug!(
            virtual_key = format_args!("{:#x}", info.vkCode),
            scan_code = format_args!("{:#x}", info.scanCode),
            extended,
            "Windows 按键没有跨平台 HID 映射，跳过"
        );
        return None;
    };
    let modifiers = runtime.modifiers.update(key_code, state == KeyState::Down);
    let injected = info.flags.contains(LLKHF_INJECTED);
    let event = InputEvent::Key {
        state,
        key_code,
        scan_code: 0,
        extended: false,
        modifiers,
    };

    Some(CapturedInput {
        event,
        timestamp_us: unix_timestamp_us(),
        injected,
        raw_relative: false,
        local_only: false,
    })
}

fn process_mouse(
    runtime: &mut HookRuntime,
    message: u32,
    info: *const MSLLHOOKSTRUCT,
) -> Option<CapturedInput> {
    if info.is_null() {
        return None;
    }

    let info = unsafe { &*info };
    let (x, y) = (info.pt.x, info.pt.y);
    let previous = runtime.last_mouse;
    let modifiers = runtime.modifiers.current();
    let injected = (info.flags & LLMHF_INJECTED) != 0;

    let event = match message {
        WM_MOUSEMOVE => {
            if runtime.relative_mode {
                return None;
            }
            let (previous_x, previous_y) = previous.unwrap_or((x, y));
            InputEvent::MouseMove {
                x,
                y,
                delta_x: x.saturating_sub(previous_x),
                delta_y: y.saturating_sub(previous_y),
                modifiers,
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let (state, button) = mouse_button(message, info.mouseData)?;
            InputEvent::MouseButton {
                state,
                button,
                x,
                y,
                modifiers,
            }
        }
        WM_MOUSEWHEEL => InputEvent::MouseWheel {
            delta_x: 0,
            delta_y: signed_high_word(info.mouseData),
            x,
            y,
            modifiers,
        },
        WM_MOUSEHWHEEL => InputEvent::MouseWheel {
            delta_x: signed_high_word(info.mouseData),
            delta_y: 0,
            x,
            y,
            modifiers,
        },
        _ => return None,
    };

    runtime.last_mouse = Some((x, y));
    Some(CapturedInput {
        event,
        timestamp_us: unix_timestamp_us(),
        injected,
        raw_relative: false,
        local_only: false,
    })
}

fn mouse_button(message: u32, mouse_data: u32) -> Option<(MouseButtonState, MouseButton)> {
    let button = match message {
        WM_LBUTTONDOWN | WM_LBUTTONUP => MouseButton::Left,
        WM_RBUTTONDOWN | WM_RBUTTONUP => MouseButton::Right,
        WM_MBUTTONDOWN | WM_MBUTTONUP => MouseButton::Middle,
        WM_XBUTTONDOWN | WM_XBUTTONUP => match (mouse_data >> 16) as u16 {
            XBUTTON1 => MouseButton::Back,
            XBUTTON2 => MouseButton::Forward,
            other => MouseButton::Other(other as u8),
        },
        _ => return None,
    };
    let state = match message {
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => MouseButtonState::Down,
        _ => MouseButtonState::Up,
    };
    Some((state, button))
}

fn signed_high_word(value: u32) -> i32 {
    i32::from((value >> 16) as u16 as i16)
}

#[derive(Debug)]
struct ModifierState {
    held: [bool; 256],
    caps_lock: bool,
    num_lock: bool,
}

impl ModifierState {
    fn new(caps_lock: bool, num_lock: bool) -> Self {
        Self {
            held: [false; 256],
            caps_lock,
            num_lock,
        }
    }

    fn update(&mut self, key_code: u32, pressed: bool) -> Modifiers {
        let index = usize::try_from(key_code)
            .ok()
            .filter(|index| *index < self.held.len());
        let was_down = index.is_some_and(|index| self.held[index]);

        if pressed && !was_down {
            match key_code {
                0x39 => self.caps_lock = !self.caps_lock,
                0x53 => self.num_lock = !self.num_lock,
                _ => {}
            }
        }
        if let Some(index) = index {
            self.held[index] = pressed;
        }
        self.current()
    }

    fn current(&self) -> Modifiers {
        let mut modifiers = Modifiers::NONE;
        if self.held[0xE0] || self.held[0xE4] {
            modifiers = modifiers.union(Modifiers::CONTROL);
        }
        if self.held[0xE1] || self.held[0xE5] {
            modifiers = modifiers.union(Modifiers::SHIFT);
        }
        if self.held[0xE2] || self.held[0xE6] {
            modifiers = modifiers.union(Modifiers::ALT);
        }
        if self.held[0xE3] || self.held[0xE7] {
            modifiers = modifiers.union(Modifiers::META);
        }
        if self.caps_lock {
            modifiers = modifiers.union(Modifiers::CAPS_LOCK);
        }
        if self.num_lock {
            modifiers = modifiers.union(Modifiers::NUM_LOCK);
        }
        modifiers
    }
}

fn normalize_absolute(x: i32, y: i32, bounds: RectI) -> (i32, i32) {
    (
        normalize_axis(x, bounds.x, bounds.right().saturating_sub(1).max(bounds.x)),
        normalize_axis(y, bounds.y, bounds.bottom().saturating_sub(1).max(bounds.y)),
    )
}

fn normalize_axis(value: i32, origin: i32, maximum: i32) -> i32 {
    let extent = i64::from(maximum) - i64::from(origin);
    if extent <= 0 {
        return 0;
    }
    let local = i64::from(value.clamp(origin, maximum)) - i64::from(origin);
    let normalized = (local * 65_535 + extent / 2) / extent;
    normalized.clamp(0, 65_535) as i32
}

pub(crate) struct WindowsInputInjector {
    pressed_keys: HashMap<u32, PressedKey>,
    pressed_buttons: Vec<MouseButton>,
    virtual_bounds: Option<(RectI, Instant)>,
}

#[derive(Clone, Copy)]
struct PressedKey {
    key_code: u32,
}

impl WindowsInputInjector {
    pub(crate) fn new() -> Self {
        Self {
            pressed_keys: HashMap::new(),
            pressed_buttons: Vec::new(),
            virtual_bounds: None,
        }
    }

    fn desktop_bounds(&mut self) -> Result<RectI> {
        if let Some((bounds, cached_at)) = self.virtual_bounds {
            if cached_at.elapsed() < VIRTUAL_BOUNDS_TTL {
                return Ok(bounds);
            }
        }
        let bounds = nc_platform::virtual_bounds()?;
        self.virtual_bounds = Some((bounds, Instant::now()));
        Ok(bounds)
    }

    fn inject_key(&mut self, state: KeyState, key_code: u32) -> Result<()> {
        let input = keyboard_input(state, key_code)?;
        send_inputs(&[input])?;

        let key = PressedKey { key_code };
        match state {
            KeyState::Down => {
                self.pressed_keys.insert(key_code, key);
            }
            KeyState::Up => {
                self.pressed_keys.remove(&key_code);
            }
        }
        Ok(())
    }

    fn inject_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
        let bounds = self.desktop_bounds()?;
        let (absolute_x, absolute_y) = normalize_absolute(x, y, bounds);
        let input = mouse_input(
            MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE,
            absolute_x,
            absolute_y,
            0,
        );
        send_inputs(&[input])
    }

    fn inject_mouse_button(&mut self, state: MouseButtonState, button: MouseButton) -> Result<()> {
        let (flags, mouse_data) = mouse_button_flags(state, button)?;
        send_inputs(&[mouse_input(flags, 0, 0, mouse_data)])?;

        match state {
            MouseButtonState::Down => {
                if !self.pressed_buttons.contains(&button) {
                    self.pressed_buttons.push(button);
                }
            }
            MouseButtonState::Up => {
                self.pressed_buttons.retain(|pressed| *pressed != button);
            }
        }
        Ok(())
    }

    fn inject_mouse_wheel(&mut self, delta_x: i32, delta_y: i32) -> Result<()> {
        if delta_x != 0 {
            send_inputs(&[mouse_input(MOUSEEVENTF_HWHEEL, 0, 0, delta_x as u32)])?;
        }
        if delta_y != 0 {
            send_inputs(&[mouse_input(MOUSEEVENTF_WHEEL, 0, 0, delta_y as u32)])?;
        }
        Ok(())
    }

    fn release_pressed_keys(&mut self) -> Result<()> {
        let mut first_error = None;
        let keys = self.pressed_keys.values().copied().collect::<Vec<_>>();
        for key in keys {
            let released =
                keyboard_input(KeyState::Up, key.key_code).and_then(|input| send_inputs(&[input]));
            match released {
                Ok(()) => {
                    self.pressed_keys.remove(&key.key_code);
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn release_pressed_buttons(&mut self) -> Result<()> {
        let mut first_error = None;
        let buttons = self.pressed_buttons.clone();
        for button in buttons {
            let released =
                mouse_button_flags(MouseButtonState::Up, button).and_then(|(flags, mouse_data)| {
                    send_inputs(&[mouse_input(flags, 0, 0, mouse_data)])
                });
            match released {
                Ok(()) => {
                    self.pressed_buttons.retain(|pressed| *pressed != button);
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl InputInjector for WindowsInputInjector {
    fn inject(&mut self, event: &InputEvent) -> Result<()> {
        match event {
            InputEvent::Key {
                state, key_code, ..
            } => self.inject_key(*state, *key_code),
            InputEvent::MouseMove { x, y, .. } => self.inject_mouse_move(*x, *y),
            InputEvent::MouseButton { state, button, .. } => {
                self.inject_mouse_button(*state, *button)
            }
            InputEvent::MouseWheel {
                delta_x, delta_y, ..
            } => self.inject_mouse_wheel(*delta_x, *delta_y),
            InputEvent::ReleaseAll => self.release_all(),
        }
    }

    fn release_all(&mut self) -> Result<()> {
        let keys = self.release_pressed_keys();
        let buttons = self.release_pressed_buttons();
        keys.and(buttons)
    }

    fn reset_desktop(&mut self) {
        self.virtual_bounds = None;
    }
}

impl Drop for WindowsInputInjector {
    fn drop(&mut self) {
        if let Err(error) = self.release_all() {
            tracing::warn!(%error, "释放注入器按下的输入失败");
        }
    }
}

fn keyboard_input(state: KeyState, key_code: u32) -> Result<INPUT> {
    let key = crate::keymap::windows_from_hid(key_code).ok_or_else(|| {
        NexusError::invalid(format!("没有对应的 Windows 键位：HID {key_code:#x}"))
    })?;

    let mut flags = KEYBD_EVENT_FLAGS(0);
    let virtual_key = if key.scan_code == 0 {
        VIRTUAL_KEY(key.virtual_key)
    } else {
        flags |= KEYEVENTF_SCANCODE;
        VIRTUAL_KEY(0)
    };
    if key.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if state == KeyState::Up {
        flags |= KEYEVENTF_KEYUP;
    }

    Ok(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: key.scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    })
}

fn mouse_button_flags(
    state: MouseButtonState,
    button: MouseButton,
) -> Result<(MOUSE_EVENT_FLAGS, u32)> {
    let down = state == MouseButtonState::Down;
    let (flags, data) = match button {
        MouseButton::Left => (
            if down {
                MOUSEEVENTF_LEFTDOWN
            } else {
                MOUSEEVENTF_LEFTUP
            },
            0,
        ),
        MouseButton::Right => (
            if down {
                MOUSEEVENTF_RIGHTDOWN
            } else {
                MOUSEEVENTF_RIGHTUP
            },
            0,
        ),
        MouseButton::Middle => (
            if down {
                MOUSEEVENTF_MIDDLEDOWN
            } else {
                MOUSEEVENTF_MIDDLEUP
            },
            0,
        ),
        MouseButton::Back | MouseButton::Forward | MouseButton::Other(1..=2) => {
            let xbutton = match button {
                MouseButton::Back | MouseButton::Other(1) => u32::from(XBUTTON1),
                _ => u32::from(XBUTTON2),
            };
            (
                if down {
                    MOUSEEVENTF_XDOWN
                } else {
                    MOUSEEVENTF_XUP
                },
                xbutton << 16,
            )
        }
        MouseButton::Other(other) => {
            return Err(NexusError::invalid(format!(
                "不支持的鼠标按键：Other({other})"
            )));
        }
    };
    Ok((flags, data))
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, x: i32, y: i32, mouse_data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: x,
                dy: y,
                mouseData: mouse_data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    arm_injection_guard();
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    arm_injection_guard();
    if sent as usize == inputs.len() {
        return Ok(());
    }

    let last_error = unsafe { GetLastError() }.0;
    Err(NexusError::platform(format!(
        "SendInput 失败：已发送 {sent}/{} 条，GetLastError={last_error}",
        inputs.len()
    )))
}
