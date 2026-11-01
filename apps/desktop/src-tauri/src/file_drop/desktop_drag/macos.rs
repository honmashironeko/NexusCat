use std::ffi::{c_char, c_void, CStr};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use super::session::{DragObservation, DragSessionTracker};
use super::{DragSignal, SignalEmitter};

const POLL_INTERVAL: Duration = Duration::from_millis(120);
const COMBINED_SESSION_STATE: i32 = 0;
const LEFT_MOUSE_BUTTON: u32 = 0;
const DRAG_PASTEBOARD_NAME: &CStr = c"Apple CFPasteboard drag";
const FILE_URL_TYPE: &CStr = c"public.file-url";
const FILENAMES_TYPE: &CStr = c"NSFilenamesPboardType";
const MAX_TYPES: usize = 64;

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *const c_void;
    fn objc_msgSend(receiver: *mut c_void, selector: *const c_void) -> isize;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

type MsgSendOne = unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> isize;
type MsgSendIndex = unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> isize;

unsafe fn send_one(receiver: *mut c_void, selector: *const c_void, argument: *mut c_void) -> isize {
    let call: MsgSendOne = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { call(receiver, selector, argument) }
}

unsafe fn send_index(receiver: *mut c_void, selector: *const c_void, index: usize) -> isize {
    let call: MsgSendIndex = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { call(receiver, selector, index) }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
}

pub fn spawn(sender: UnboundedSender<DragSignal>) {
    let _ = thread::Builder::new()
        .name("nc-desktop-drag".into())
        .spawn(move || {
            let mut emitter = SignalEmitter::new(sender);
            let mut tracker = DragSessionTracker::new(observe().change_count);
            loop {
                thread::sleep(POLL_INTERVAL);
                let active = tracker.update(observe());
                if !emitter.update(active) {
                    break;
                }
            }
        });
}

fn observe() -> DragObservation {
    let pool = unsafe { objc_autoreleasePoolPush() };
    let button_down =
        unsafe { CGEventSourceButtonState(COMBINED_SESSION_STATE, LEFT_MOUSE_BUTTON) };
    let change_count = unsafe { drag_pasteboard_change_count() };
    let has_files = button_down && change_count.is_some() && unsafe { drag_pasteboard_has_files() };
    unsafe { objc_autoreleasePoolPop(pool) };
    DragObservation {
        button_down,
        change_count,
        has_files,
    }
}

unsafe fn class(name: &CStr) -> Option<*mut c_void> {
    let pointer = unsafe { objc_getClass(name.as_ptr()) };
    (!pointer.is_null()).then_some(pointer)
}

unsafe fn selector(name: &CStr) -> *const c_void {
    unsafe { sel_registerName(name.as_ptr()) }
}

unsafe fn ns_string(text: &CStr) -> *mut c_void {
    let Some(string_class) = (unsafe { class(c"NSString") }) else {
        return std::ptr::null_mut();
    };
    let value = unsafe {
        send_one(
            string_class,
            selector(c"stringWithUTF8String:"),
            text.as_ptr() as *mut c_void,
        )
    };
    value as *mut c_void
}

unsafe fn drag_pasteboard() -> Option<*mut c_void> {
    let pasteboard_class = unsafe { class(c"NSPasteboard") }?;
    let name = unsafe { ns_string(DRAG_PASTEBOARD_NAME) };
    if name.is_null() {
        return None;
    }
    let pasteboard = unsafe { send_one(pasteboard_class, selector(c"pasteboardWithName:"), name) }
        as *mut c_void;
    (!pasteboard.is_null()).then_some(pasteboard)
}

unsafe fn drag_pasteboard_change_count() -> Option<i64> {
    let pasteboard = unsafe { drag_pasteboard() }?;
    let count = unsafe { objc_msgSend(pasteboard, selector(c"changeCount")) };
    Some(count as i64)
}

unsafe fn drag_pasteboard_has_files() -> bool {
    let Some(pasteboard) = (unsafe { drag_pasteboard() }) else {
        return false;
    };
    let types = unsafe { objc_msgSend(pasteboard, selector(c"types")) } as *mut c_void;
    if types.is_null() {
        return false;
    }
    let count = unsafe { objc_msgSend(types, selector(c"count")) } as usize;
    let file_url = unsafe { ns_string(FILE_URL_TYPE) };
    let filenames = unsafe { ns_string(FILENAMES_TYPE) };
    if file_url.is_null() || filenames.is_null() {
        return false;
    }
    let is_equal = unsafe { selector(c"isEqualToString:") };
    let object_at = unsafe { selector(c"objectAtIndex:") };
    for index in 0..count.min(MAX_TYPES) {
        let item = unsafe { send_index(types, object_at, index) } as *mut c_void;
        if item.is_null() {
            continue;
        }
        if unsafe { send_one(item, is_equal, file_url) } as u8 != 0 {
            return true;
        }
        if unsafe { send_one(item, is_equal, filenames) } as u8 != 0 {
            return true;
        }
    }
    false
}
