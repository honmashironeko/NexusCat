use nc_core::{NexusError, PermissionKind, PermissionReport, Result};

use crate::{GpuInfo, MonitorInfo, RectI};

fn unsupported() -> NexusError {
    NexusError::platform(format!("当前平台不受支持：{}", std::env::consts::OS))
}

pub(crate) fn monitors() -> Result<Vec<MonitorInfo>> {
    Err(unsupported())
}

pub(crate) fn gpu_info() -> Result<GpuInfo> {
    Err(unsupported())
}

pub(crate) fn cursor_position() -> Result<(i32, i32)> {
    Err(unsupported())
}

pub(crate) fn set_cursor_position(_x: i32, _y: i32) -> Result<()> {
    Err(unsupported())
}

pub(crate) fn set_cursor_clip(_bounds: Option<RectI>) -> Result<()> {
    Err(unsupported())
}

pub(crate) fn secure_desktop_active() -> bool {
    true
}

pub(crate) fn set_idle_lock_prevention(_active: bool) -> Result<()> {
    Ok(())
}

pub(crate) fn permission_report() -> Result<PermissionReport> {
    Err(unsupported())
}

pub(crate) fn request_permission(_kind: PermissionKind) -> Result<bool> {
    Err(unsupported())
}

pub(crate) fn device_name() -> String {
    "NexusCat".to_string()
}
