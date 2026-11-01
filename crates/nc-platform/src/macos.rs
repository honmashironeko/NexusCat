#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;

use nc_core::{
    NexusError, PermissionKind, PermissionReport, PermissionStatus, PhysicalSize, Result,
};

use crate::{GpuInfo, MonitorInfo, RectI};

const IOHID_REQUEST_LISTEN_EVENT: u32 = 1;
const IOHID_ACCESS_GRANTED: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CgPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CgSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CgRect {
    origin: CgPoint,
    size: CgSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(
        max_displays: u32,
        displays: *mut u32,
        display_count: *mut u32,
    ) -> i32;
    fn CGDisplayBounds(display: u32) -> CgRect;
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
    fn CGDisplayModeGetPixelWidth(mode: *const c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *const c_void) -> usize;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
    fn CGDisplayScreenSize(display: u32) -> CgSize;
    fn CGDisplayRotation(display: u32) -> f64;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsMain(display: u32) -> u32;
    fn CGEventCreate(source: *mut c_void) -> *mut c_void;
    fn CGEventGetLocation(event: *mut c_void) -> CgPoint;
    fn CGWarpMouseCursorPosition(point: CgPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: u32) -> i32;
    fn CGPreflightScreenCaptureAccess() -> u8;
    fn CGRequestScreenCaptureAccess() -> u8;
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
    fn CGDisplayIOServicePort(display: u32) -> u32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: *const c_void;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize,
        key_call_backs: *const c_void,
        value_call_backs: *const c_void,
    ) -> *const c_void;
    fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        c_str: *const c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFAllocatorGetDefault() -> *const c_void;
    fn CFBooleanGetValue(boolean: *const c_void) -> u8;
    fn CFGetTypeID(value: *const c_void) -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetCString(
        string: *const c_void,
        buffer: *mut c_char,
        buffer_size: isize,
        encoding: u32,
    ) -> u8;
    fn CFRelease(value: *const c_void);
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    static kAXTrustedCheckOptionPrompt: *const c_void;
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDCheckAccess(request_type: u32) -> u32;
    fn IOHIDRequestAccess(request_type: u32) -> u8;
    fn IODisplayCreateInfoDictionary(service: u32, options: u32) -> *const c_void;
    fn IOPMAssertionCreateWithName(
        assertion_type: *const c_void,
        assertion_level: u32,
        assertion_name: *const c_void,
        assertion_id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(assertion_id: u32) -> i32;
}

unsafe extern "C" {
    fn gethostname(name: *mut c_char, len: usize) -> c_int;
}

pub(crate) fn monitors() -> Result<Vec<MonitorInfo>> {
    let mut count = 0u32;
    let status = unsafe { CGGetActiveDisplayList(0, ptr::null_mut(), &mut count) };
    if status != 0 {
        return Err(NexusError::platform(format!(
            "CGGetActiveDisplayList 获取显示器数量失败：CGError {status}"
        )));
    }

    let mut displays = vec![0u32; count as usize];
    if count > 0 {
        let mut actual = 0u32;
        let status = unsafe { CGGetActiveDisplayList(count, displays.as_mut_ptr(), &mut actual) };
        if status != 0 {
            return Err(NexusError::platform(format!(
                "CGGetActiveDisplayList 枚举显示器失败：CGError {status}"
            )));
        }
        displays.truncate(actual.min(count) as usize);
    }

    let main_display = unsafe { CGMainDisplayID() };
    displays
        .iter()
        .map(|display| monitor_info(*display, main_display))
        .collect()
}

fn monitor_info(display: u32, main_display: u32) -> Result<MonitorInfo> {
    let bounds = rect_from_cg(unsafe { CGDisplayBounds(display) })?;
    let screen_size = unsafe { CGDisplayScreenSize(display) };
    let physical = if screen_size.width > 0.0
        && screen_size.height > 0.0
        && screen_size.width.is_finite()
        && screen_size.height.is_finite()
    {
        Some(PhysicalSize::new(
            screen_size.width as f32,
            screen_size.height as f32,
        ))
    } else {
        None
    };

    let pixel_width = unsafe { CGDisplayPixelsWide(display) };
    let pixel_height = unsafe { CGDisplayPixelsHigh(display) };
    let (pixel_width, pixel_height) =
        display_mode_pixel_size(display).unwrap_or((pixel_width, pixel_height));
    let is_main = unsafe { CGDisplayIsMain(display) } != 0;
    let rotation_degrees = normalize_rotation(unsafe { CGDisplayRotation(display) });

    Ok(MonitorInfo {
        id: display,
        name: display_name(display),
        bounds,
        work_area: bounds,
        dpi: average_dpi(pixel_width, pixel_height, screen_size),
        primary: is_main || display == main_display,
        physical,
        rotation_degrees,
        pixel_width: usize_to_u32(pixel_width),
        pixel_height: usize_to_u32(pixel_height),
    })
}

fn display_name(display: u32) -> String {
    let service = unsafe { CGDisplayIOServicePort(display) };
    if service == 0 {
        return format!("Display {display}");
    }
    let dictionary = unsafe { IODisplayCreateInfoDictionary(service, 0) };
    if dictionary.is_null() {
        return format!("Display {display}");
    }

    let key_name = CString::new("DisplayProductName").ok();
    let key = key_name
        .as_ref()
        .map(|name| unsafe {
            CFStringCreateWithCString(
                CFAllocatorGetDefault(),
                name.as_ptr(),
                CF_STRING_ENCODING_UTF8,
            )
        })
        .unwrap_or(ptr::null());
    let value = if key.is_null() {
        ptr::null()
    } else {
        unsafe { CFDictionaryGetValue(dictionary, key) }
    };
    let name = if value.is_null() || unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() }
    {
        None
    } else {
        let mut buffer = [0_i8; 512];
        let copied = unsafe {
            CFStringGetCString(
                value,
                buffer.as_mut_ptr(),
                buffer.len() as isize,
                CF_STRING_ENCODING_UTF8,
            )
        };
        if copied == 0 {
            None
        } else {
            Some(
                unsafe { CStr::from_ptr(buffer.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    };

    if !key.is_null() {
        unsafe { CFRelease(key) };
    }
    unsafe { CFRelease(dictionary) };
    name.filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Display {display}"))
}

fn normalize_rotation(degrees: f64) -> u16 {
    if !degrees.is_finite() {
        return 0;
    }
    match (degrees.rem_euclid(360.0).round() as u32) % 360 {
        90 => 90,
        180 => 180,
        270 => 270,
        _ => 0,
    }
}

fn rect_from_cg(rect: CgRect) -> Result<RectI> {
    let origin_x = rect.origin.x.round();
    let origin_y = rect.origin.y.round();
    let width = rect.size.width.round();
    let height = rect.size.height.round();

    if !origin_x.is_finite()
        || !origin_y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width < 0.0
        || height < 0.0
    {
        return Err(NexusError::platform("CGDisplayBounds 返回了非法矩形"));
    }

    Ok(RectI {
        x: clamp_i32(origin_x),
        y: clamp_i32(origin_y),
        width: clamp_u32(width),
        height: clamp_u32(height),
    })
}

fn clamp_i32(value: f64) -> i32 {
    value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn clamp_u32(value: f64) -> u32 {
    value.clamp(0.0, f64::from(u32::MAX)) as u32
}

fn usize_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn display_mode_pixel_size(display: u32) -> Option<(usize, usize)> {
    let mode = unsafe { CGDisplayCopyDisplayMode(display) };
    if mode.is_null() {
        return None;
    }
    let mut width = unsafe { CGDisplayModeGetPixelWidth(mode) };
    let mut height = unsafe { CGDisplayModeGetPixelHeight(mode) };
    unsafe { CFRelease(mode) };
    if width == 0 || height == 0 {
        return None;
    }
    let desktop_width = unsafe { CGDisplayPixelsWide(display) };
    let desktop_height = unsafe { CGDisplayPixelsHigh(display) };
    if (width >= height) != (desktop_width >= desktop_height) {
        (width, height) = (height, width);
    }
    Some((width, height))
}

fn average_dpi(pixel_width: usize, pixel_height: usize, screen_size: CgSize) -> u32 {
    let horizontal = dpi_axis(pixel_width, screen_size.width);
    let vertical = dpi_axis(pixel_height, screen_size.height);
    match (horizontal, vertical) {
        (Some(horizontal), Some(vertical)) => {
            let average = (horizontal + vertical) / 2.0;
            (average.round().clamp(1.0, f64::from(u32::MAX))) as u32
        }
        _ => 96,
    }
}

fn dpi_axis(pixels: usize, millimeters: f64) -> Option<f64> {
    if pixels > 0 && millimeters > 0.0 && millimeters.is_finite() {
        Some(pixels as f64 * 25.4 / millimeters)
    } else {
        None
    }
}

pub(crate) fn cursor_position() -> Result<(i32, i32)> {
    let event = unsafe { CGEventCreate(ptr::null_mut()) };
    if event.is_null() {
        return Err(NexusError::platform("CGEventCreate 返回空事件"));
    }

    let location = unsafe { CGEventGetLocation(event) };
    unsafe { CFRelease(event as *const c_void) };

    if !location.x.is_finite() || !location.y.is_finite() {
        return Err(NexusError::platform("CGEventGetLocation 返回非法坐标"));
    }

    Ok((clamp_i32(location.x.round()), clamp_i32(location.y.round())))
}

pub(crate) fn set_cursor_clip(_bounds: Option<RectI>) -> Result<()> {
    Ok(())
}

pub(crate) fn set_cursor_position(x: i32, y: i32) -> Result<()> {
    let point = CgPoint {
        x: f64::from(x),
        y: f64::from(y),
    };
    let status = unsafe { CGWarpMouseCursorPosition(point) };
    if status != 0 {
        return Err(NexusError::platform(format!(
            "CGWarpMouseCursorPosition 失败：CGError {status}"
        )));
    }

    let status = unsafe { CGAssociateMouseAndMouseCursorPosition(1) };
    if status != 0 {
        return Err(NexusError::platform(format!(
            "CGAssociateMouseAndMouseCursorPosition 失败：CGError {status}"
        )));
    }
    Ok(())
}

pub(crate) fn secure_desktop_active() -> bool {
    let secure_input = unsafe { IsSecureEventInputEnabled() } != 0;
    let screen_locked = unsafe {
        let dictionary = CGSessionCopyCurrentDictionary();
        if dictionary.is_null() {
            true
        } else {
            let key_name = CString::new("CGSSessionScreenIsLocked").ok();
            let key = key_name
                .as_ref()
                .map(|name| {
                    CFStringCreateWithCString(
                        CFAllocatorGetDefault(),
                        name.as_ptr(),
                        CF_STRING_ENCODING_UTF8,
                    )
                })
                .unwrap_or(ptr::null());
            let locked = if key.is_null() {
                false
            } else {
                let value = CFDictionaryGetValue(dictionary, key);
                let locked = if value.is_null() {
                    false
                } else if CFGetTypeID(value) == CFBooleanGetTypeID() {
                    CFBooleanGetValue(value) != 0
                } else {
                    false
                };
                CFRelease(key);
                locked
            };
            CFRelease(dictionary);
            locked
        }
    };
    secure_input || screen_locked
}

pub(crate) fn permission_report() -> Result<PermissionReport> {
    Ok(PermissionReport {
        statuses: vec![
            PermissionStatus::new(PermissionKind::InputMonitoring, input_monitoring_granted()),
            PermissionStatus::new(PermissionKind::Accessibility, accessibility_granted()),
            PermissionStatus::new(PermissionKind::ScreenRecording, screen_recording_granted()),
        ],
    })
}

pub(crate) fn request_permission(kind: PermissionKind) -> Result<bool> {
    match kind {
        PermissionKind::ScreenRecording => {
            let _ = unsafe { CGRequestScreenCaptureAccess() };
            Ok(screen_recording_granted())
        }
        PermissionKind::Accessibility => {
            request_accessibility_prompt();
            Ok(accessibility_granted())
        }
        PermissionKind::InputMonitoring => {
            let _ = unsafe { IOHIDRequestAccess(IOHID_REQUEST_LISTEN_EVENT) };
            Ok(input_monitoring_granted())
        }
    }
}

fn screen_recording_granted() -> bool {
    if (unsafe { CGPreflightScreenCaptureAccess() }) != 0 {
        return true;
    }
    screen_content_accessible()
}

fn screen_content_accessible() -> bool {
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const TTL_MICROS: i64 = 3_000_000;
    static LAST_CONFIRMED_US: AtomicI64 = AtomicI64::new(0);

    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros() as i64)
        .unwrap_or(0);
    if now_us.saturating_sub(LAST_CONFIRMED_US.load(Ordering::Relaxed)) < TTL_MICROS {
        return true;
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(screencapturekit::prelude::SCShareableContent::get().is_ok());
    });
    match rx.recv_timeout(Duration::from_millis(1500)) {
        Ok(true) => {
            LAST_CONFIRMED_US.store(now_us, Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

fn accessibility_granted() -> bool {
    (unsafe { AXIsProcessTrusted() }) != 0
}

fn input_monitoring_granted() -> bool {
    (unsafe { IOHIDCheckAccess(IOHID_REQUEST_LISTEN_EVENT) }) == IOHID_ACCESS_GRANTED
}

fn request_accessibility_prompt() {
    let keys = [unsafe { kAXTrustedCheckOptionPrompt }];
    let values = [unsafe { kCFBooleanTrue }];
    let options = unsafe {
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const c_void,
        )
    };
    if options.is_null() {
        tracing::debug!("CFDictionaryCreate 失败，无法弹出辅助功能授权提示");
        return;
    }

    let _ = unsafe { AXIsProcessTrustedWithOptions(options) };
    unsafe { CFRelease(options) };
}

pub(crate) fn set_idle_lock_prevention(active: bool) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static ASSERTION_IDS: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();

    if active == ENABLED.swap(active, Ordering::SeqCst) {
        return Ok(());
    }
    let ids = ASSERTION_IDS.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = ids
        .lock()
        .map_err(|_| NexusError::platform("闲置锁屏防护状态锁中毒"))?;
    if !active {
        for id in guard.drain(..) {
            let _ = unsafe { IOPMAssertionRelease(id) };
        }
        return Ok(());
    }
    for assertion_type in ["PreventUserIdleDisplaySleep", "NoIdleSleep"] {
        let type_cf = cf_string(assertion_type);
        let name_cf = cf_string("NexusCat remote session");
        let mut id: u32 = 0;
        let status = unsafe { IOPMAssertionCreateWithName(type_cf, 255, name_cf, &mut id) };
        unsafe {
            CFRelease(type_cf);
            CFRelease(name_cf);
        }
        if status == 0 && id != 0 {
            guard.push(id);
        }
    }
    if guard.is_empty() {
        tracing::warn!("IOPMAssertion 创建失败，闲置锁屏防护未生效");
    }
    Ok(())
}

fn cf_string(text: &str) -> *const c_void {
    let c_string = CString::new(text).expect("cf_string 文本不得包含 NUL");
    unsafe {
        CFStringCreateWithCString(
            CFAllocatorGetDefault(),
            c_string.as_ptr(),
            CF_STRING_ENCODING_UTF8,
        )
    }
}

pub(crate) fn gpu_info() -> Result<GpuInfo> {
    Ok(GpuInfo {
        name: "Apple / macOS".to_string(),
        integrated: true,
        dedicated_video_memory_mb: 0,
    })
}

pub(crate) fn device_name() -> String {
    let mut buffer: [c_char; 256] = [0; 256];
    let status = unsafe { gethostname(buffer.as_mut_ptr(), buffer.len()) };
    if status != 0 {
        return "NexusCat".to_string();
    }

    buffer[buffer.len() - 1] = 0;
    let hostname = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if hostname.trim().is_empty() {
        "NexusCat".to_string()
    } else {
        hostname
    }
}
