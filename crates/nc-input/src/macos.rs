#![allow(non_camel_case_types, non_snake_case)]

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, SendTimeoutError, Sender};
use nc_core::{InputEvent, KeyState, Modifiers, MouseButton, MouseButtonState, NexusError, Result};

use crate::{unix_timestamp_us, CapturedInput, InputCapture, InputInjector};

const KCG_SESSION_EVENT_TAP: u32 = 1;
const KCG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const KCG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const KCG_HID_EVENT_TAP: u32 = 0;
const KCG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const KCG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

const KCG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const KCG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const KCG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const KCG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
const KCG_EVENT_MOUSE_MOVED: u32 = 5;
const KCG_EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const KCG_EVENT_RIGHT_MOUSE_DRAGGED: u32 = 7;
const KCG_EVENT_KEY_DOWN: u32 = 10;
const KCG_EVENT_KEY_UP: u32 = 11;
const KCG_EVENT_FLAGS_CHANGED: u32 = 12;
const KCG_EVENT_SCROLL_WHEEL: u32 = 22;
const KCG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const KCG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const KCG_EVENT_OTHER_MOUSE_DRAGGED: u32 = 27;

const KCG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const KCG_MOUSE_EVENT_BUTTON_NUMBER: u32 = 3;
const KCG_MOUSE_EVENT_DELTA_X: u32 = 4;
const KCG_MOUSE_EVENT_DELTA_Y: u32 = 5;
const KCG_SCROLL_WHEEL_EVENT_DELTA_AXIS_1: u32 = 11;
const KCG_SCROLL_WHEEL_EVENT_DELTA_AXIS_2: u32 = 12;
const KCG_EVENT_SOURCE_UNIX_PROCESS_ID: u32 = 41;
const KCG_EVENT_SOURCE_USER_DATA: u32 = 42;

const KCG_SCROLL_EVENT_UNIT_LINE: u32 = 1;
const KCG_MOUSE_BUTTON_LEFT: u32 = 0;
const KCG_MOUSE_BUTTON_RIGHT: u32 = 1;
const KCG_MOUSE_BUTTON_CENTER: u32 = 2;

const KCG_EVENT_FLAG_MASK_ALPHA_SHIFT: u64 = 0x0001_0000;
const KCG_EVENT_FLAG_MASK_SHIFT: u64 = 0x0002_0000;
const KCG_EVENT_FLAG_MASK_CONTROL: u64 = 0x0004_0000;
const KCG_EVENT_FLAG_MASK_ALTERNATE: u64 = 0x0008_0000;
const KCG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;

const EVENT_MASK: u64 = (1 << KCG_EVENT_LEFT_MOUSE_DOWN)
    | (1 << KCG_EVENT_LEFT_MOUSE_UP)
    | (1 << KCG_EVENT_RIGHT_MOUSE_DOWN)
    | (1 << KCG_EVENT_RIGHT_MOUSE_UP)
    | (1 << KCG_EVENT_MOUSE_MOVED)
    | (1 << KCG_EVENT_LEFT_MOUSE_DRAGGED)
    | (1 << KCG_EVENT_RIGHT_MOUSE_DRAGGED)
    | (1 << KCG_EVENT_KEY_DOWN)
    | (1 << KCG_EVENT_KEY_UP)
    | (1 << KCG_EVENT_FLAGS_CHANGED)
    | (1 << KCG_EVENT_SCROLL_WHEEL)
    | (1 << KCG_EVENT_OTHER_MOUSE_DOWN)
    | (1 << KCG_EVENT_OTHER_MOUSE_UP)
    | (1 << KCG_EVENT_OTHER_MOUSE_DRAGGED);

const INJECTION_USER_DATA: i64 = 0x4E43_4154;
const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const SINK_SEND_TIMEOUT: Duration = Duration::from_millis(10);
const JOIN_TIMEOUT: Duration = Duration::from_secs(2);
const TAP_READY_TIMEOUT: Duration = Duration::from_secs(5);

type CGEventTapCallBack =
    unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: u8);
    fn CGEventGetFlags(event: *mut c_void) -> u64;
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    fn CGEventSetIntegerValueField(event: *mut c_void, field: u32, value: i64);
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventSetLocation(event: *mut c_void, location: CGPoint);
    fn CGEventCreateKeyboardEvent(
        source: *mut c_void,
        virtual_key: u16,
        key_down: u8,
    ) -> *mut c_void;
    fn CGEventCreateMouseEvent(
        source: *mut c_void,
        mouse_type: u32,
        mouse_cursor_position: CGPoint,
        mouse_button: u32,
    ) -> *mut c_void;
    fn CGEventCreateScrollWheelEvent2(
        source: *mut c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> *mut c_void;
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CGEventSourceCreate(state_id: u32) -> *mut c_void;
    fn CGEventSourceSetLocalEventsSuppressionInterval(source: *mut c_void, seconds: f64);
    fn CGEventSourceSetUserData(source: *mut c_void, user_data: i64);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: *mut c_void,
        order: isize,
    ) -> *mut c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRemoveSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRunInMode(
        mode: *const c_void,
        seconds: f64,
        return_after_source_handled: u8,
    ) -> i32;
    fn CFRunLoopStop(run_loop: *mut c_void);
    fn CFRelease(value: *const c_void);
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        c_str: *const std::os::raw::c_char,
        encoding: u32,
    ) -> *mut c_void;
    fn CFPreferencesCopyAppValue(key: *const c_void, application_id: *const c_void) -> *mut c_void;
    fn CFBooleanGetValue(boolean: *const c_void) -> u8;
}

unsafe extern "C" {
    fn getpid() -> i32;
}

pub(crate) struct MacosInputCapture {
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
    join: JoinHandle<()>,
    run_loop: usize,
    stop_requested: Arc<AtomicBool>,
}

impl MacosInputCapture {
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

impl InputCapture for MacosInputCapture {
    fn start(&mut self, sink: Sender<CapturedInput>) -> Result<()> {
        let mut state = self.lock_state();
        if self.running.load(Ordering::Acquire) {
            return Err(NexusError::Business("输入捕获已经在运行".to_string()));
        }
        if let Some(worker) = state.worker.take() {
            join_with_timeout(worker.join);
        }

        self.running.store(true, Ordering::Release);
        let stop_requested = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) =
            crossbeam_channel::bounded::<std::result::Result<usize, String>>(1);
        let suppressed = Arc::clone(&self.suppressed);
        let running = Arc::clone(&self.running);
        let relative_mode = Arc::clone(&self.relative_mode);
        let absolute_pointer = Arc::clone(&self.absolute_pointer);
        let stop_signal = Arc::clone(&stop_requested);

        let join = match thread::Builder::new()
            .name("nc-input-macos-tap".to_string())
            .spawn(move || {
                capture_thread(
                    sink,
                    suppressed,
                    running,
                    relative_mode,
                    absolute_pointer,
                    stop_signal,
                    ready_tx,
                )
            }) {
            Ok(join) => join,
            Err(error) => {
                self.running.store(false, Ordering::Release);
                return Err(NexusError::platform(format!(
                    "启动 macOS 输入捕获线程失败：{error}"
                )));
            }
        };

        match ready_rx.recv_timeout(TAP_READY_TIMEOUT) {
            Ok(Ok(run_loop)) => {
                state.worker = Some(CaptureWorker {
                    join,
                    run_loop,
                    stop_requested,
                });
                Ok(())
            }
            Ok(Err(message)) => {
                join_with_timeout(join);
                self.running.store(false, Ordering::Release);
                Err(NexusError::platform(message))
            }
            Err(RecvTimeoutError::Timeout) => {
                join_with_timeout(join);
                self.running.store(false, Ordering::Release);
                Err(NexusError::platform(
                    "macOS 输入捕获初始化超时：可能仍在等待系统授权".to_string(),
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                join_with_timeout(join);
                self.running.store(false, Ordering::Release);
                Err(NexusError::platform(
                    "macOS 输入捕获线程启动时退出".to_string(),
                ))
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
            worker.stop_requested.store(true, Ordering::Release);
            unsafe {
                CFRunLoopStop(worker.run_loop as *mut c_void);
            }
            join_with_timeout(worker.join);
        }
    }

    fn set_suppressed(&self, suppressed: bool) {
        self.suppressed.store(suppressed, Ordering::Release);
    }

    fn is_suppressed(&self) -> bool {
        self.suppressed.load(Ordering::Acquire)
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn set_relative_mode(&self, enabled: bool) {
        self.relative_mode.store(enabled, Ordering::Release);
    }

    fn set_absolute_pointer(&self, enabled: bool) {
        self.absolute_pointer.store(enabled, Ordering::Release);
    }
}

impl Drop for MacosInputCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn join_with_timeout(join: JoinHandle<()>) {
    let deadline = Instant::now() + JOIN_TIMEOUT;
    while !join.is_finished() {
        if Instant::now() >= deadline {
            tracing::warn!("等待 macOS 输入捕获线程退出超时，线程将被分离");
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    if join.join().is_err() {
        tracing::warn!("macOS 输入捕获线程异常结束");
    }
}

struct RunningGuard(Arc<AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct MacTapContext {
    sink: Sender<CapturedInput>,
    suppressed: Arc<AtomicBool>,
    relative_mode: Arc<AtomicBool>,
    absolute_pointer: Arc<AtomicBool>,
    tap: Cell<*mut c_void>,
}

fn capture_thread(
    sink: Sender<CapturedInput>,
    suppressed: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    relative_mode: Arc<AtomicBool>,
    absolute_pointer: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
    ready: Sender<std::result::Result<usize, String>>,
) {
    let _running_guard = RunningGuard(running);
    let context = Box::into_raw(Box::new(MacTapContext {
        sink,
        suppressed,
        relative_mode,
        absolute_pointer,
        tap: Cell::new(ptr::null_mut()),
    }));

    let tap = unsafe {
        CGEventTapCreate(
            KCG_SESSION_EVENT_TAP,
            KCG_HEAD_INSERT_EVENT_TAP,
            KCG_EVENT_TAP_OPTION_DEFAULT,
            EVENT_MASK,
            event_tap_callback,
            context.cast(),
        )
    };
    if tap.is_null() {
        unsafe {
            drop(Box::from_raw(context));
        }
        let _ = ready.send(Err(
            "CGEventTapCreate 失败；请确认已授予辅助功能/输入监控权限".to_string(),
        ));
        return;
    }
    unsafe {
        (*context).tap.set(tap);
    }

    let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };
    if source.is_null() {
        unsafe {
            CGEventTapEnable(tap, 0);
            CFRelease(tap as *const c_void);
            drop(Box::from_raw(context));
        }
        let _ = ready.send(Err("CFMachPortCreateRunLoopSource 失败".to_string()));
        return;
    }

    let run_loop = unsafe { CFRunLoopGetCurrent() };
    unsafe {
        CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
    }

    if ready.send(Ok(run_loop as usize)).is_err() {
        cleanup_tap(run_loop, source, tap, context);
        return;
    }

    while !stop_requested.load(Ordering::Acquire) {
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 0);
        }
    }

    cleanup_tap(run_loop, source, tap, context);
}

fn cleanup_tap(
    run_loop: *mut c_void,
    source: *mut c_void,
    tap: *mut c_void,
    context: *mut MacTapContext,
) {
    unsafe {
        CGEventTapEnable(tap, 0);
        CFRunLoopRemoveSource(run_loop, source, kCFRunLoopCommonModes);
        CFRelease(source as *const c_void);
        CFRelease(tap as *const c_void);
        drop(Box::from_raw(context));
    }
}

unsafe extern "C" fn event_tap_callback(
    _proxy: *mut c_void,
    event_type: u32,
    event: *mut c_void,
    user_info: *mut c_void,
) -> *mut c_void {
    if user_info.is_null() {
        return event;
    }

    let context = unsafe { &*(user_info as *const MacTapContext) };
    if event_type == KCG_EVENT_TAP_DISABLED_BY_TIMEOUT
        || event_type == KCG_EVENT_TAP_DISABLED_BY_USER_INPUT
    {
        let tap = context.tap.get();
        if !tap.is_null() {
            unsafe { CGEventTapEnable(tap, 1) };
        }
        return event;
    }
    if event.is_null() {
        return event;
    }

    let Some((input, injected)) = (unsafe { event_to_input(event_type, event) }) else {
        return event;
    };
    let is_move = matches!(input, InputEvent::MouseMove { .. });
    let raw_relative = !injected && is_move && context.relative_mode.load(Ordering::Acquire);
    let captured = CapturedInput {
        event: input,
        timestamp_us: unix_timestamp_us(),
        injected,
        raw_relative,
        local_only: false,
    };
    let allow_move = is_move && context.absolute_pointer.load(Ordering::Acquire);
    let suppress = !injected && context.suppressed.load(Ordering::Acquire) && !allow_move;
    let delivered = send_captured(&context.sink, &context.suppressed, captured);

    if suppress && delivered {
        ptr::null_mut()
    } else {
        event
    }
}

fn send_captured(
    sink: &Sender<CapturedInput>,
    suppressed: &AtomicBool,
    input: CapturedInput,
) -> bool {
    match sink.send_timeout(input, SINK_SEND_TIMEOUT) {
        Ok(()) => true,
        Err(SendTimeoutError::Timeout(_)) => {
            tracing::debug!("输入事件接收端繁忙，丢弃本条 macOS 事件");
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

unsafe fn event_to_input(event_type: u32, event: *mut c_void) -> Option<(InputEvent, bool)> {
    let injected = is_injected_event(event);
    let modifiers = modifiers_from_flags(unsafe { CGEventGetFlags(event) });
    let event = match event_type {
        KCG_EVENT_KEY_DOWN | KCG_EVENT_KEY_UP => {
            let mac_key_code = field_u32(event, KCG_KEYBOARD_EVENT_KEYCODE)?;
            let key_code = crate::keymap::hid_from_macos(mac_key_code)?;
            let state = if event_type == KCG_EVENT_KEY_DOWN {
                KeyState::Down
            } else {
                KeyState::Up
            };
            InputEvent::Key {
                state,
                key_code,
                scan_code: 0,
                extended: false,
                modifiers,
            }
        }
        KCG_EVENT_FLAGS_CHANGED => {
            let mac_key_code = field_u32(event, KCG_KEYBOARD_EVENT_KEYCODE)?;
            let flags = unsafe { CGEventGetFlags(event) };
            let state = modifier_key_state(mac_key_code, flags)?;
            let key_code = crate::keymap::hid_from_macos(mac_key_code)?;
            InputEvent::Key {
                state,
                key_code,
                scan_code: 0,
                extended: false,
                modifiers,
            }
        }
        KCG_EVENT_MOUSE_MOVED
        | KCG_EVENT_LEFT_MOUSE_DRAGGED
        | KCG_EVENT_RIGHT_MOUSE_DRAGGED
        | KCG_EVENT_OTHER_MOUSE_DRAGGED => {
            let (x, y) = event_location(event)?;
            InputEvent::MouseMove {
                x,
                y,
                delta_x: field_i32(event, KCG_MOUSE_EVENT_DELTA_X),
                delta_y: field_i32(event, KCG_MOUSE_EVENT_DELTA_Y),
                modifiers,
            }
        }
        KCG_EVENT_LEFT_MOUSE_DOWN
        | KCG_EVENT_LEFT_MOUSE_UP
        | KCG_EVENT_RIGHT_MOUSE_DOWN
        | KCG_EVENT_RIGHT_MOUSE_UP
        | KCG_EVENT_OTHER_MOUSE_DOWN
        | KCG_EVENT_OTHER_MOUSE_UP => {
            let (x, y) = event_location(event)?;
            let (state, button) = mouse_button_event(event_type, event)?;
            InputEvent::MouseButton {
                state,
                button,
                x,
                y,
                modifiers,
            }
        }
        KCG_EVENT_SCROLL_WHEEL => {
            let (x, y) = event_location(event)?;
            InputEvent::MouseWheel {
                delta_x: wheel_delta(unsafe {
                    CGEventGetIntegerValueField(event, KCG_SCROLL_WHEEL_EVENT_DELTA_AXIS_2)
                }),
                delta_y: wheel_delta(unsafe {
                    CGEventGetIntegerValueField(event, KCG_SCROLL_WHEEL_EVENT_DELTA_AXIS_1)
                }),
                x,
                y,
                modifiers,
            }
        }
        _ => return None,
    };

    Some((event, injected))
}

fn event_location(event: *mut c_void) -> Option<(i32, i32)> {
    let point = unsafe { CGEventGetLocation(event) };
    if !point.x.is_finite() || !point.y.is_finite() {
        return None;
    }
    Some((
        point
            .x
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
        point
            .y
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
    ))
}

fn field_u32(event: *mut c_void, field: u32) -> Option<u32> {
    let value = unsafe { CGEventGetIntegerValueField(event, field) };
    u32::try_from(value).ok()
}

fn field_i32(event: *mut c_void, field: u32) -> i32 {
    let value = unsafe { CGEventGetIntegerValueField(event, field) };
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn mouse_button_event(
    event_type: u32,
    event: *mut c_void,
) -> Option<(MouseButtonState, MouseButton)> {
    let down = matches!(
        event_type,
        KCG_EVENT_LEFT_MOUSE_DOWN | KCG_EVENT_RIGHT_MOUSE_DOWN | KCG_EVENT_OTHER_MOUSE_DOWN
    );
    let button = match event_type {
        KCG_EVENT_LEFT_MOUSE_DOWN | KCG_EVENT_LEFT_MOUSE_UP => MouseButton::Left,
        KCG_EVENT_RIGHT_MOUSE_DOWN | KCG_EVENT_RIGHT_MOUSE_UP => MouseButton::Right,
        KCG_EVENT_OTHER_MOUSE_DOWN | KCG_EVENT_OTHER_MOUSE_UP => {
            let number =
                unsafe { CGEventGetIntegerValueField(event, KCG_MOUSE_EVENT_BUTTON_NUMBER) };
            match number {
                2 => MouseButton::Middle,
                3 => MouseButton::Back,
                4 => MouseButton::Forward,
                other => MouseButton::Other(other.clamp(0, 255) as u8),
            }
        }
        _ => return None,
    };
    Some((
        if down {
            MouseButtonState::Down
        } else {
            MouseButtonState::Up
        },
        button,
    ))
}

fn is_injected_event(event: *mut c_void) -> bool {
    let user_data = unsafe { CGEventGetIntegerValueField(event, KCG_EVENT_SOURCE_USER_DATA) };
    if user_data == INJECTION_USER_DATA {
        return true;
    }
    let source_pid =
        unsafe { CGEventGetIntegerValueField(event, KCG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    source_pid != 0 && source_pid == i64::from(unsafe { getpid() })
}

fn modifiers_from_flags(flags: u64) -> Modifiers {
    let mut modifiers = Modifiers::NONE;
    if flags & KCG_EVENT_FLAG_MASK_SHIFT != 0 {
        modifiers = modifiers.union(Modifiers::SHIFT);
    }
    if flags & KCG_EVENT_FLAG_MASK_CONTROL != 0 {
        modifiers = modifiers.union(Modifiers::CONTROL);
    }
    if flags & KCG_EVENT_FLAG_MASK_ALTERNATE != 0 {
        modifiers = modifiers.union(Modifiers::ALT);
    }
    if flags & KCG_EVENT_FLAG_MASK_COMMAND != 0 {
        modifiers = modifiers.union(Modifiers::META);
    }
    if flags & KCG_EVENT_FLAG_MASK_ALPHA_SHIFT != 0 {
        modifiers = modifiers.union(Modifiers::CAPS_LOCK);
    }
    modifiers
}

fn cg_flags_from_modifiers(modifiers: Modifiers) -> u64 {
    let mut flags = 0;
    if modifiers.shift() {
        flags |= KCG_EVENT_FLAG_MASK_SHIFT;
    }
    if modifiers.control() {
        flags |= KCG_EVENT_FLAG_MASK_CONTROL;
    }
    if modifiers.alt() {
        flags |= KCG_EVENT_FLAG_MASK_ALTERNATE;
    }
    if modifiers.meta() {
        flags |= KCG_EVENT_FLAG_MASK_COMMAND;
    }
    if modifiers.contains(Modifiers::CAPS_LOCK) {
        flags |= KCG_EVENT_FLAG_MASK_ALPHA_SHIFT;
    }
    flags
}

fn modifier_key_state(key_code: u32, flags: u64) -> Option<KeyState> {
    let mask = match key_code {
        56 | 60 => KCG_EVENT_FLAG_MASK_SHIFT,
        59 | 62 => KCG_EVENT_FLAG_MASK_CONTROL,
        58 | 61 => KCG_EVENT_FLAG_MASK_ALTERNATE,
        54 | 55 => KCG_EVENT_FLAG_MASK_COMMAND,
        57 => KCG_EVENT_FLAG_MASK_ALPHA_SHIFT,
        _ => return None,
    };
    Some(if flags & mask != 0 {
        KeyState::Down
    } else {
        KeyState::Up
    })
}

fn wheel_delta(lines: i64) -> i32 {
    let scaled = lines.saturating_mul(120);
    scaled.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn wheel_lines(delta: i32) -> i32 {
    if delta == 0 {
        return 0;
    }
    let lines = delta / 120;
    if lines == 0 {
        delta.signum()
    } else {
        lines
    }
}

fn natural_scrolling_enabled() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, value)) = *cache {
        if at.elapsed() < Duration::from_secs(2) {
            return value;
        }
    }
    let value = read_natural_scrolling_pref();
    *cache = Some((Instant::now(), value));
    value
}

fn read_natural_scrolling_pref() -> bool {
    unsafe {
        let key = CFStringCreateWithCString(
            ptr::null(),
            c"com.apple.swipescrolldirection".as_ptr(),
            KCF_STRING_ENCODING_UTF8,
        );
        let domain = CFStringCreateWithCString(
            ptr::null(),
            c".GlobalPreferences".as_ptr(),
            KCF_STRING_ENCODING_UTF8,
        );
        if key.is_null() || domain.is_null() {
            if !key.is_null() {
                CFRelease(key);
            }
            if !domain.is_null() {
                CFRelease(domain);
            }
            return false;
        }
        let value = CFPreferencesCopyAppValue(key, domain);
        CFRelease(key);
        CFRelease(domain);
        if value.is_null() {
            return false;
        }
        let enabled = CFBooleanGetValue(value) != 0;
        CFRelease(value);
        enabled
    }
}

pub(crate) struct MacosInputInjector {
    pressed_keys: Vec<u16>,
    pressed_buttons: Vec<MouseButton>,
    source: usize,
}

impl MacosInputInjector {
    pub(crate) fn new() -> Result<Self> {
        let source = unsafe {
            let source = CGEventSourceCreate(1);
            if source.is_null() {
                tracing::warn!("CGEventSourceCreate 失败，注入事件将无法标记来源");
            } else {
                CGEventSourceSetLocalEventsSuppressionInterval(source, 0.0);
                CGEventSourceSetUserData(source, INJECTION_USER_DATA);
            }
            source as usize
        };
        Ok(Self {
            pressed_keys: Vec::new(),
            pressed_buttons: Vec::new(),
            source,
        })
    }

    fn source(&self) -> *mut c_void {
        self.source as *mut c_void
    }

    fn inject_key(&mut self, state: KeyState, key_code: u32, modifiers: Modifiers) -> Result<()> {
        let Some(key_code) = crate::keymap::macos_from_hid(key_code) else {
            tracing::debug!(
                key_code = format_args!("{key_code:#x}"),
                "macOS 没有对应的按键，忽略"
            );
            return Ok(());
        };
        let event = unsafe {
            CGEventCreateKeyboardEvent(self.source(), key_code, u8::from(state == KeyState::Down))
        };
        if event.is_null() {
            return Err(NexusError::platform(
                "CGEventCreateKeyboardEvent 返回空事件",
            ));
        }
        unsafe {
            CGEventSetFlags(event, cg_flags_from_modifiers(modifiers));
        }
        post_event(event);

        match state {
            KeyState::Down => {
                if !self.pressed_keys.contains(&key_code) {
                    self.pressed_keys.push(key_code);
                }
            }
            KeyState::Up => {
                self.pressed_keys.retain(|pressed| *pressed != key_code);
            }
        }
        Ok(())
    }

    fn inject_mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
        let (event_type, button_number) = self
            .pressed_buttons
            .last()
            .map(|button| drag_event_type(*button))
            .unwrap_or((KCG_EVENT_MOUSE_MOVED, KCG_MOUSE_BUTTON_LEFT));
        let event = unsafe {
            CGEventCreateMouseEvent(self.source(), event_type, point(x, y), button_number)
        };
        if event.is_null() {
            return Err(NexusError::platform("CGEventCreateMouseEvent 返回空事件"));
        }
        post_event(event);
        Ok(())
    }

    fn inject_mouse_button(
        &mut self,
        state: MouseButtonState,
        button: MouseButton,
        x: i32,
        y: i32,
    ) -> Result<()> {
        let (event_type, button_number) = mouse_event_type(state, button)?;
        let event = unsafe {
            CGEventCreateMouseEvent(self.source(), event_type, point(x, y), button_number)
        };
        if event.is_null() {
            return Err(NexusError::platform("CGEventCreateMouseEvent 返回空事件"));
        }
        post_event(event);

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

    fn inject_mouse_wheel(&mut self, x: i32, y: i32, delta_x: i32, delta_y: i32) -> Result<()> {
        let mut vertical = wheel_lines(delta_y);
        let mut horizontal = wheel_lines(delta_x);
        if vertical == 0 && horizontal == 0 {
            return Ok(());
        }
        if natural_scrolling_enabled() {
            vertical = -vertical;
            horizontal = -horizontal;
        }
        let wheel_count = if horizontal != 0 { 2 } else { 1 };
        let event = unsafe {
            CGEventCreateScrollWheelEvent2(
                self.source(),
                KCG_SCROLL_EVENT_UNIT_LINE,
                wheel_count,
                vertical,
                horizontal,
                0,
            )
        };
        if event.is_null() {
            return Err(NexusError::platform(
                "CGEventCreateScrollWheelEvent2 返回空事件",
            ));
        }
        unsafe {
            CGEventSetLocation(event, point(x, y));
        }
        post_event(event);
        Ok(())
    }

    fn release_pressed_keys(&mut self) -> Result<()> {
        let keys = self.pressed_keys.clone();
        for key_code in keys {
            let event = unsafe { CGEventCreateKeyboardEvent(self.source(), key_code, 0) };
            if event.is_null() {
                return Err(NexusError::platform(
                    "CGEventCreateKeyboardEvent 返回空事件",
                ));
            }
            post_event(event);
            self.pressed_keys.retain(|pressed| *pressed != key_code);
        }
        Ok(())
    }

    fn release_pressed_buttons(&mut self) -> Result<()> {
        let buttons = self.pressed_buttons.clone();
        let (x, y) = nc_platform::cursor_position().unwrap_or((0, 0));
        for button in buttons {
            let (event_type, button_number) = mouse_event_type(MouseButtonState::Up, button)?;
            let event = unsafe {
                CGEventCreateMouseEvent(self.source(), event_type, point(x, y), button_number)
            };
            if event.is_null() {
                return Err(NexusError::platform("CGEventCreateMouseEvent 返回空事件"));
            }
            post_event(event);
            self.pressed_buttons.retain(|pressed| *pressed != button);
        }
        Ok(())
    }
}

impl InputInjector for MacosInputInjector {
    fn inject(&mut self, event: &InputEvent) -> Result<()> {
        match event {
            InputEvent::Key {
                state,
                key_code,
                modifiers,
                ..
            } => self.inject_key(*state, *key_code, *modifiers),
            InputEvent::MouseMove { x, y, .. } => self.inject_mouse_move(*x, *y),
            InputEvent::MouseButton {
                state,
                button,
                x,
                y,
                ..
            } => self.inject_mouse_button(*state, *button, *x, *y),
            InputEvent::MouseWheel {
                delta_x,
                delta_y,
                x,
                y,
                ..
            } => self.inject_mouse_wheel(*x, *y, *delta_x, *delta_y),
            InputEvent::ReleaseAll => self.release_all(),
        }
    }

    fn release_all(&mut self) -> Result<()> {
        self.release_pressed_keys()?;
        self.release_pressed_buttons()
    }
}

impl Drop for MacosInputInjector {
    fn drop(&mut self) {
        if let Err(error) = self.release_all() {
            tracing::warn!(%error, "释放 macOS 注入器按下的输入失败");
        }
        if self.source != 0 {
            unsafe { CFRelease(self.source as *const c_void) };
            self.source = 0;
        }
    }
}

fn point(x: i32, y: i32) -> CGPoint {
    CGPoint {
        x: f64::from(x),
        y: f64::from(y),
    }
}

fn drag_event_type(button: MouseButton) -> (u32, u32) {
    match button {
        MouseButton::Left => (KCG_EVENT_LEFT_MOUSE_DRAGGED, KCG_MOUSE_BUTTON_LEFT),
        MouseButton::Right => (KCG_EVENT_RIGHT_MOUSE_DRAGGED, KCG_MOUSE_BUTTON_RIGHT),
        MouseButton::Middle => (KCG_EVENT_OTHER_MOUSE_DRAGGED, KCG_MOUSE_BUTTON_CENTER),
        MouseButton::Back => (KCG_EVENT_OTHER_MOUSE_DRAGGED, 3),
        MouseButton::Forward => (KCG_EVENT_OTHER_MOUSE_DRAGGED, 4),
        MouseButton::Other(number) => (KCG_EVENT_OTHER_MOUSE_DRAGGED, u32::from(number)),
    }
}

fn mouse_event_type(state: MouseButtonState, button: MouseButton) -> Result<(u32, u32)> {
    let down = state == MouseButtonState::Down;
    let event_type = match button {
        MouseButton::Left => {
            if down {
                KCG_EVENT_LEFT_MOUSE_DOWN
            } else {
                KCG_EVENT_LEFT_MOUSE_UP
            }
        }
        MouseButton::Right => {
            if down {
                KCG_EVENT_RIGHT_MOUSE_DOWN
            } else {
                KCG_EVENT_RIGHT_MOUSE_UP
            }
        }
        MouseButton::Middle | MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => {
            if down {
                KCG_EVENT_OTHER_MOUSE_DOWN
            } else {
                KCG_EVENT_OTHER_MOUSE_UP
            }
        }
    };
    let button_number = match button {
        MouseButton::Left => KCG_MOUSE_BUTTON_LEFT,
        MouseButton::Right => KCG_MOUSE_BUTTON_RIGHT,
        MouseButton::Middle => KCG_MOUSE_BUTTON_CENTER,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
        MouseButton::Other(number) => u32::from(number),
    };
    Ok((event_type, button_number))
}

fn post_event(event: *mut c_void) {
    unsafe {
        CGEventSetIntegerValueField(event, KCG_EVENT_SOURCE_USER_DATA, INJECTION_USER_DATA);
        CGEventPost(KCG_HID_EVENT_TAP, event);
        CFRelease(event as *const c_void);
    }
}
