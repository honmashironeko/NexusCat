use std::ffi::c_void;
use std::mem::size_of;

use nc_core::{NexusError, PermissionKind, PermissionReport, PhysicalSize, Result};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    ERROR_MORE_DATA, ERROR_SUCCESS, HANDLE, LPARAM, POINT, RECT, TRUE,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayDevicesW, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW,
    DEVMODE_DISPLAY_ORIENTATION, DISPLAY_DEVICEW, DMDO_180, DMDO_270, DMDO_90,
    ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    KEY_WOW64_64KEY,
};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_ACCESS_FLAGS,
    DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP, UOI_NAME,
};
use windows::Win32::System::WindowsProgramming::GetComputerNameW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    ClipCursor, GetCursorPos, SetCursorPos, EDD_GET_DEVICE_INTERFACE_NAME, MONITORINFOF_PRIMARY,
};

use crate::{GpuInfo, MonitorInfo, RectI};

const EDID_VALUE_NAME: &str = "EDID";
const DEVICE_NAME_FALLBACK: &str = "NexusCat";

#[derive(Default)]
struct MonitorEnumeration {
    monitors: Vec<MonitorInfo>,
    error: Option<String>,
}

pub(crate) fn monitors() -> Result<Vec<MonitorInfo>> {
    let mut enumeration = MonitorEnumeration::default();
    let data = LPARAM((&mut enumeration as *mut MonitorEnumeration) as isize);

    let succeeded = unsafe { EnumDisplayMonitors(None, None, Some(enum_monitor), data) };
    if !succeeded.as_bool() {
        return Err(NexusError::platform("EnumDisplayMonitors 调用失败"));
    }
    if let Some(error) = enumeration.error {
        return Err(NexusError::platform(error));
    }
    Ok(enumeration.monitors)
}

pub(crate) fn gpu_info() -> Result<GpuInfo> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
        .map_err(|error| NexusError::platform(format!("创建 DXGI 工厂失败：{error}")))?;
    let mut best: Option<GpuInfo> = None;
    let mut index = 0u32;
    loop {
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(_) => break,
        };
        index = index.saturating_add(1);
        let description = match unsafe { adapter.GetDesc1() } {
            Ok(description) => description,
            Err(error) => {
                tracing::debug!(%error, index, "读取 DXGI 适配器信息失败");
                continue;
            }
        };
        if description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let name = String::from_utf16_lossy(&description.Description)
            .trim_end_matches('\0')
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let dedicated_video_memory_mb = (description.DedicatedVideoMemory as u64) / (1024 * 1024);
        let info = GpuInfo {
            name,
            integrated: dedicated_video_memory_mb < GpuInfo::INTEGRATED_MEMORY_THRESHOLD_MB,
            dedicated_video_memory_mb,
        };
        if best.as_ref().is_none_or(|current| {
            info.dedicated_video_memory_mb > current.dedicated_video_memory_mb
        }) {
            best = Some(info);
        }
    }
    best.ok_or_else(|| NexusError::NotFound("DXGI 未返回显示适配器".into()))
}

unsafe extern "system" fn enum_monitor(
    monitor: HMONITOR,
    _device_context: HDC,
    _monitor_rect: *mut RECT,
    data: LPARAM,
) -> windows::core::BOOL {
    let enumeration = unsafe { &mut *(data.0 as *mut MonitorEnumeration) };
    let mut monitor_info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    if !unsafe { GetMonitorInfoW(monitor, &mut monitor_info.monitorInfo) }.as_bool() {
        if enumeration.error.is_none() {
            enumeration.error = Some("GetMonitorInfoW 调用失败".to_string());
        }
        return TRUE;
    }

    let device_name = utf16z_to_string(&monitor_info.szDevice);
    let (friendly_name, physical) = query_display_device(&monitor_info.szDevice);
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    if let Err(error) =
        unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
    {
        tracing::debug!(%error, monitor = %device_name, "GetDpiForMonitor 失败，按 96 DPI 处理");
    }

    let bounds = rect_from_win(&monitor_info.monitorInfo.rcMonitor);
    enumeration.monitors.push(MonitorInfo {
        id: monitor_id(monitor),
        name: friendly_name.unwrap_or(device_name),
        bounds,
        work_area: rect_from_win(&monitor_info.monitorInfo.rcWork),
        dpi: if dpi_x == 0 { 96 } else { dpi_x },
        primary: monitor_info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        physical,
        rotation_degrees: display_rotation_degrees(&monitor_info.szDevice),
        pixel_width: bounds.width as u32,
        pixel_height: bounds.height as u32,
    });
    TRUE
}

fn display_rotation_degrees(device_name: &[u16; 32]) -> u16 {
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let succeeded = unsafe {
        EnumDisplaySettingsW(
            PCWSTR(device_name.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut mode,
        )
    };
    if !succeeded.as_bool() {
        return 0;
    }
    rotation_from_orientation(unsafe { mode.Anonymous1.Anonymous2.dmDisplayOrientation })
}

fn rotation_from_orientation(orientation: DEVMODE_DISPLAY_ORIENTATION) -> u16 {
    if orientation == DMDO_90 {
        90
    } else if orientation == DMDO_180 {
        180
    } else if orientation == DMDO_270 {
        270
    } else {
        0
    }
}

fn monitor_id(monitor: HMONITOR) -> u32 {
    monitor.0 as usize as u32
}

fn rect_from_win(rect: &RECT) -> RectI {
    let width = i64::from(rect.right) - i64::from(rect.left);
    let height = i64::from(rect.bottom) - i64::from(rect.top);
    RectI {
        x: rect.left,
        y: rect.top,
        width: u32::try_from(width.max(0)).unwrap_or(0),
        height: u32::try_from(height.max(0)).unwrap_or(0),
    }
}

fn query_display_device(device_name: &[u16; 32]) -> (Option<String>, Option<PhysicalSize>) {
    let mut display = DISPLAY_DEVICEW {
        cb: size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };
    let enumerated = unsafe {
        EnumDisplayDevicesW(
            PCWSTR(device_name.as_ptr()),
            0,
            &mut display,
            EDD_GET_DEVICE_INTERFACE_NAME,
        )
    };
    if !enumerated.as_bool() {
        return (None, None);
    }

    let friendly_name = utf16z_to_string(&display.DeviceString);
    let interface_name = utf16z_to_string(&display.DeviceID);
    let instance = monitor_instance_from_interface(&interface_name);
    let edid = instance.as_deref().and_then(read_edid_bytes);
    let physical = edid.as_deref().and_then(parse_edid_physical_size);
    let name = edid
        .as_deref()
        .and_then(parse_edid_monitor_name)
        .or_else(|| (!friendly_name.is_empty()).then_some(friendly_name));
    (name, physical)
}

fn monitor_instance_from_interface(interface_name: &str) -> Option<String> {
    let path = interface_name.strip_prefix(r"\\?\")?;
    let (instance, _) = path.split_once("#{")?;
    if instance.is_empty() {
        return None;
    }
    Some(instance.replace('#', "\\"))
}

fn read_edid_bytes(instance_id: &str) -> Option<Vec<u8>> {
    if instance_id.is_empty() {
        return None;
    }

    let key_path = wide_null(&format!(
        "SYSTEM\\CurrentControlSet\\Enum\\{instance_id}\\Device Parameters"
    ));
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key_path.as_ptr()),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }

    let value_name = wide_null(EDID_VALUE_NAME);
    let mut size = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            None,
            None,
            None,
            Some(&mut size),
        )
    };
    if (status != ERROR_SUCCESS && status != ERROR_MORE_DATA) || !(128..=65536).contains(&size) {
        let _ = unsafe { RegCloseKey(key) };
        return None;
    }

    let mut edid = vec![0u8; size as usize];
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            None,
            None,
            Some(edid.as_mut_ptr()),
            Some(&mut size),
        )
    };
    let _ = unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS {
        return None;
    }

    let length = (size as usize).min(edid.len());
    Some(edid[..length].to_vec())
}

fn parse_edid_monitor_name(edid: &[u8]) -> Option<String> {
    const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if edid.len() < 128 || edid[..EDID_HEADER.len()] != EDID_HEADER {
        return None;
    }
    for descriptor_start in (54..=108).step_by(18) {
        let descriptor = &edid[descriptor_start..descriptor_start + 18];
        if descriptor[0] != 0 || descriptor[1] != 0 || descriptor[3] != 0xFC {
            continue;
        }
        let text = &descriptor[5..18];
        let end = text
            .iter()
            .position(|byte| *byte == 0x0A || *byte == 0)
            .unwrap_or(text.len());
        let name = String::from_utf8_lossy(&text[..end]).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

fn parse_edid_physical_size(edid: &[u8]) -> Option<PhysicalSize> {
    const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if edid.len() < 128 || edid[..EDID_HEADER.len()] != EDID_HEADER {
        return None;
    }

    let width_cm = edid[21];
    let height_cm = edid[22];
    if width_cm == 0 || height_cm == 0 {
        return None;
    }

    Some(PhysicalSize::new(
        f32::from(width_cm) * 10.0,
        f32::from(height_cm) * 10.0,
    ))
}

fn utf16z_to_string(units: &[u16]) -> String {
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..length])
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn cursor_position() -> Result<(i32, i32)> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }
        .map_err(|error| NexusError::platform(format!("GetCursorPos 失败：{error}")))?;
    Ok((point.x, point.y))
}

pub(crate) fn set_cursor_position(x: i32, y: i32) -> Result<()> {
    unsafe { SetCursorPos(x, y) }
        .map_err(|error| NexusError::platform(format!("SetCursorPos 失败：{error}")))
}

pub(crate) fn set_cursor_clip(bounds: Option<RectI>) -> Result<()> {
    let rect = bounds.map(|bounds| RECT {
        left: bounds.x,
        top: bounds.y,
        right: bounds.right(),
        bottom: bounds.bottom(),
    });
    unsafe { ClipCursor(rect.as_ref().map(|rect| rect as *const RECT)) }
        .map_err(|error| NexusError::platform(format!("ClipCursor 失败：{error}")))
}

pub(crate) fn set_idle_lock_prevention(_active: bool) -> Result<()> {
    Ok(())
}

pub(crate) fn secure_desktop_active() -> bool {
    let desktop = match unsafe {
        OpenInputDesktop(
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_ACCESS_FLAGS(DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0),
        )
    } {
        Ok(desktop) => desktop,
        Err(error) => {
            tracing::debug!(%error, "OpenInputDesktop 失败，按安全桌面处理");
            return true;
        }
    };

    let mut buffer = [0u16; 256];
    let result = unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            (buffer.len() * size_of::<u16>()) as u32,
            None,
        )
    };
    let _ = unsafe { CloseDesktop(desktop) };

    match result {
        Ok(()) => {
            let name = utf16z_to_string(&buffer);
            !name.eq_ignore_ascii_case("Default")
        }
        Err(error) => {
            tracing::debug!(%error, "GetUserObjectInformationW 失败，按安全桌面处理");
            true
        }
    }
}

pub(crate) fn permission_report() -> Result<PermissionReport> {
    Ok(PermissionReport::default())
}

pub(crate) fn request_permission(_kind: PermissionKind) -> Result<bool> {
    Ok(true)
}

pub(crate) fn device_name() -> String {
    let mut buffer = [0u16; 256];
    let mut size = buffer.len() as u32;
    match unsafe { GetComputerNameW(Some(PWSTR(buffer.as_mut_ptr())), &mut size) } {
        Ok(()) => {
            let length = (size as usize).min(buffer.len());
            let name = String::from_utf16_lossy(&buffer[..length]);
            if name.trim().is_empty() {
                DEVICE_NAME_FALLBACK.to_string()
            } else {
                name
            }
        }
        Err(error) => {
            tracing::debug!(%error, "GetComputerNameW 失败，回退设备名");
            DEVICE_NAME_FALLBACK.to_string()
        }
    }
}

