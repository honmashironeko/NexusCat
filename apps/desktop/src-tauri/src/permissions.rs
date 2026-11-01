use nc_core::{NexusError, PermissionKind, PermissionReport};
use tauri::AppHandle;

use crate::config::AppPaths;

pub fn report() -> PermissionReport {
    match nc_platform::permission_report() {
        Ok(report) => report,
        Err(error) => {
            tracing::warn!(%error, "权限体检失败");
            PermissionReport::default()
        }
    }
}

pub fn request(kind: PermissionKind) -> Result<bool, NexusError> {
    nc_platform::request_permission(kind)
}

pub fn auto_request_missing_once(app: &AppHandle, paths: &AppPaths) {
    let marker = paths.data_dir.join("permission_auto_requested");
    if marker.exists() {
        return;
    }
    let _ = std::fs::write(&marker, b"requested");

    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let missing = nc_platform::permission_report()
            .map(|report| report.missing())
            .unwrap_or_else(|_| PermissionKind::required_for_platform().to_vec());
        let ordered: Vec<PermissionKind> = [
            PermissionKind::InputMonitoring,
            PermissionKind::ScreenRecording,
            PermissionKind::Accessibility,
        ]
        .into_iter()
        .filter(|kind| missing.contains(kind))
        .collect();
        if ordered.is_empty() {
            return;
        }
        let _ = app.run_on_main_thread(move || {
            for kind in ordered {
                let _ = nc_platform::request_permission(kind);
            }
        });
    });
}

pub fn all_granted() -> bool {
    report().all_granted()
}

pub fn open_settings(app: &AppHandle, kind: PermissionKind) -> Result<(), NexusError> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_opener::OpenerExt;

        let pane = match kind {
            PermissionKind::ScreenRecording => "Privacy_ScreenCapture",
            PermissionKind::Accessibility => "Privacy_Accessibility",
            PermissionKind::InputMonitoring => "Privacy_ListenEvent",
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|error| NexusError::platform(format!("打开系统设置失败：{error}")))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, kind);
        Err(NexusError::platform("仅 macOS 提供系统权限设置入口"))
    }
}
