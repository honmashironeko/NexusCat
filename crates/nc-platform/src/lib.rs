#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;
#[cfg(windows)]
mod win32;

#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(not(any(windows, target_os = "macos")))]
use self::unsupported as imp;
#[cfg(windows)]
use self::win32 as imp;

use nc_core::{NexusError, PermissionKind, PermissionReport, PhysicalSize, Result};
use serde::{Deserialize, Serialize};

#[cfg(windows)]
pub struct TimerResolutionGuard;

#[cfg(windows)]
impl TimerResolutionGuard {
    pub fn acquire() -> Self {
        unsafe {
            windows::Win32::Media::timeBeginPeriod(1);
        }
        Self
    }
}

#[cfg(windows)]
impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::Media::timeEndPeriod(1);
        }
    }
}

#[cfg(not(windows))]
#[derive(Debug, Clone, Copy, Default)]
pub struct TimerResolutionGuard;

#[cfg(not(windows))]
impl TimerResolutionGuard {
    pub fn acquire() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> i32 {
        i32::try_from(i64::from(self.x) + i64::from(self.width)).unwrap_or(i32::MAX)
    }

    pub fn bottom(&self) -> i32 {
        i32::try_from(i64::from(self.y) + i64::from(self.height)).unwrap_or(i32::MAX)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub bounds: RectI,
    pub work_area: RectI,
    pub dpi: u32,
    pub primary: bool,
    pub physical: Option<PhysicalSize>,
    #[serde(default)]
    pub rotation_degrees: u16,
    #[serde(default)]
    pub pixel_width: u32,
    #[serde(default)]
    pub pixel_height: u32,
}

impl MonitorInfo {
    pub const fn capture_size(&self) -> (u32, u32) {
        if self.pixel_width > 0 && self.pixel_height > 0 {
            (self.pixel_width, self.pixel_height)
        } else {
            (self.bounds.width, self.bounds.height)
        }
    }
}

impl MonitorInfo {
    pub fn is_quarter_turned(&self) -> bool {
        self.rotation_degrees % 180 == 90
    }

    pub fn display_physical(&self) -> PhysicalSize {
        let physical = self.physical.unwrap_or_default();
        if self.is_quarter_turned() {
            PhysicalSize::new(physical.height_mm, physical.width_mm)
        } else {
            physical
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub hostname: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub integrated: bool,
    pub dedicated_video_memory_mb: u64,
}

impl GpuInfo {
    pub fn unknown() -> Self {
        Self {
            name: String::new(),
            integrated: true,
            dedicated_video_memory_mb: 0,
        }
    }

    pub const INTEGRATED_MEMORY_THRESHOLD_MB: u64 = 512;
}

#[cfg(windows)]
pub fn enable_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

#[cfg(not(windows))]
pub fn enable_dpi_awareness() {}

pub fn monitors() -> Result<Vec<MonitorInfo>> {
    imp::monitors()
}

pub fn primary_monitor() -> Result<Option<MonitorInfo>> {
    Ok(monitors()?.into_iter().find(|monitor| monitor.primary))
}

pub fn virtual_bounds() -> Result<RectI> {
    union_bounds(&monitors()?).ok_or_else(|| NexusError::platform("未枚举到任何显示器"))
}

pub fn gpu_info() -> GpuInfo {
    imp::gpu_info().unwrap_or_else(|error| {
        tracing::debug!(%error, "GPU 探测失败，按集成显卡处理");
        GpuInfo::unknown()
    })
}

pub fn cursor_position() -> Result<(i32, i32)> {
    imp::cursor_position()
}

pub fn set_cursor_position(x: i32, y: i32) -> Result<()> {
    imp::set_cursor_position(x, y)
}

pub fn set_cursor_clip(bounds: Option<RectI>) -> Result<()> {
    imp::set_cursor_clip(bounds)
}

pub fn secure_desktop_active() -> bool {
    imp::secure_desktop_active()
}

pub fn set_idle_lock_prevention(active: bool) -> Result<()> {
    imp::set_idle_lock_prevention(active)
}

pub fn permission_report() -> Result<PermissionReport> {
    imp::permission_report()
}

pub fn request_permission(kind: PermissionKind) -> Result<bool> {
    imp::request_permission(kind)
}

pub fn device_name() -> String {
    imp::device_name()
}

pub fn platform_info() -> PlatformInfo {
    PlatformInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        hostname: device_name(),
    }
}

pub fn is_supported() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

fn union_bounds(monitors: &[MonitorInfo]) -> Option<RectI> {
    let mut left = i64::MAX;
    let mut top = i64::MAX;
    let mut right = i64::MIN;
    let mut bottom = i64::MIN;

    for monitor in monitors {
        let x = i64::from(monitor.bounds.x);
        let y = i64::from(monitor.bounds.y);
        left = left.min(x);
        top = top.min(y);
        right = right.max(x + i64::from(monitor.bounds.width));
        bottom = bottom.max(y + i64::from(monitor.bounds.height));
    }

    if left > right || top > bottom {
        return None;
    }

    Some(RectI {
        x: i32::try_from(left).ok()?,
        y: i32::try_from(top).ok()?,
        width: u32::try_from(right - left).ok()?,
        height: u32::try_from(bottom - top).ok()?,
    })
}

