use crate::actions::UiAction;
use crate::config;
use crate::ipc::{
    Bootstrap, CommandError, DeviceView, FileDropCardView, FileDropCardsView,
    InputServiceStatusView, MediaBridgeInfo, TransferView, UpdateStatusView, WebCodecsProbe,
};
use crate::orchestrator as orch;
use crate::runtime::AppState;
use crate::window;
use nc_core::{
    AppNotification, AppSettings, DeviceId, DisplayNodeId, PermissionKind, PermissionReport,
    SessionSnapshot, StreamId,
};
use nc_observability::MetricsSnapshot;
use nc_topology::Layout;
use std::sync::Arc;
use tauri::State;

type AppStateRef<'a> = State<'a, Arc<AppState>>;

fn parse_device_id(text: &str) -> Result<DeviceId, CommandError> {
    DeviceId::from_hex(text.trim()).map_err(|error| CommandError::new("invalid_input", error))
}

fn parse_permission(text: &str) -> Result<PermissionKind, CommandError> {
    match text {
        "screen_recording" => Ok(PermissionKind::ScreenRecording),
        "accessibility" => Ok(PermissionKind::Accessibility),
        "input_monitoring" => Ok(PermissionKind::InputMonitoring),
        other => Err(CommandError::new(
            "invalid_input",
            format!("未知权限类型：{other}"),
        )),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_bootstrap(state: AppStateRef<'_>) -> Result<Bootstrap, CommandError> {
    let state = state.inner();
    let settings = state.settings.read().clone();
    Ok(Bootstrap {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: nc_core::Platform::current(),
        data_dir: state.paths.data_dir.display().to_string(),
        settings,
        session: state.snapshot(),
        devices: state.device_views(),
        layout: state.layout.read().clone(),
        notifications: state.notifications.lock().list(),
        transfers: state.transfers.lock().views(),
        permissions: state.permissions.read().clone(),
        metrics: state.metrics.snapshot(),
        media_bridge: state.bridge_info.clone(),
        webcodecs: state.webcodecs.read().clone(),
        video_capabilities: crate::ipc::VideoCapabilitiesView::from_runtime(
            &state.video_capabilities,
        ),
        shared_config: crate::orchestrator::shared_config_status(state),
        group_status: crate::orchestrator::group_status(state),
        test_mode: state.test_mode,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_state(state: AppStateRef<'_>) -> SessionSnapshot {
    state.inner().snapshot()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_devices(state: AppStateRef<'_>) -> Vec<DeviceView> {
    state.inner().device_views()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_layout(state: AppStateRef<'_>) -> Layout {
    state.inner().layout.read().clone()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_settings(state: AppStateRef<'_>) -> AppSettings {
    state.inner().settings.read().clone()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_transfers(state: AppStateRef<'_>) -> Vec<TransferView> {
    state.inner().transfers.lock().views()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_notifications(state: AppStateRef<'_>) -> Vec<AppNotification> {
    state.inner().notifications.lock().list()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_metrics(state: AppStateRef<'_>) -> MetricsSnapshot {
    state.inner().metrics.snapshot()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_permissions(state: AppStateRef<'_>) -> PermissionReport {
    state.inner().permissions.read().clone()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_update_status(state: AppStateRef<'_>) -> UpdateStatusView {
    state.inner().update.lock().status.clone()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_media_bridge(state: AppStateRef<'_>) -> MediaBridgeInfo {
    state.inner().bridge_info.clone()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_discovery(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::start_discovery_command(&state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stop_discovery(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::stop_discovery_command(&state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn probe_devices(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::probe_discovery(&state)
        .await
        .map_err(CommandError::internal)
}

#[tauri::command(rename_all = "snake_case")]
pub fn pair_device(state: AppStateRef<'_>, device_id: String) -> Result<(), CommandError> {
    let device = parse_device_id(&device_id)?;
    orch::begin_pairing(state.inner(), device);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn confirm_pairing(state: AppStateRef<'_>, accept: bool) -> Result<(), CommandError> {
    orch::confirm_pairing(state.inner(), accept).map_err(CommandError::from)
}

#[tauri::command(rename_all = "snake_case")]
pub fn sync_shared_config(state: AppStateRef<'_>) -> Result<(), CommandError> {
    crate::orchestrator::force_shared_config_sync(state.inner());
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_pairing(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::cancel_pairing(&state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remove_trusted_device(
    state: AppStateRef<'_>,
    device_id: String,
) -> Result<(), CommandError> {
    let device = parse_device_id(&device_id)?;
    let state = Arc::clone(state.inner());
    orch::remove_trusted_device(&state, device)
        .await
        .map_err(CommandError::from)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn connect_device(state: AppStateRef<'_>, device_id: String) -> Result<(), CommandError> {
    let device = parse_device_id(&device_id)?;
    let state = Arc::clone(state.inner());
    orch::begin_connect(&state, device)
        .await
        .map_err(CommandError::from)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn disconnect(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::disconnect_command(&state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_mode(state: AppStateRef<'_>, mode: String) -> Result<(), CommandError> {
    let mode = mode
        .parse::<nc_core::Mode>()
        .map_err(|error| CommandError::new("invalid_input", error))?;
    let state = Arc::clone(state.inner());
    orch::set_mode(&state, mode).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn toggle_mode(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::toggle_mode(&state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn summon_stage(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::summon_stage(&state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn hide_stage(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::hide_stage(&state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_layout(state: AppStateRef<'_>, layout: Layout) -> Result<Layout, CommandError> {
    layout.validate().map_err(CommandError::from)?;
    let current = state.inner().layout.read().clone();
    let mut layout = layout;
    layout.active_node_ids = current.active_node_ids.clone();
    layout.remote_desktop_origins = current.remote_desktop_origins.clone();
    let mut changed = layout.apply_resolution_changes(&current.nodes);
    let incoming: std::collections::HashSet<_> =
        layout.nodes.iter().map(|node| node.id.clone()).collect();
    for node in current.nodes {
        if !incoming.contains(&node.id) {
            layout.nodes.push(node);
        }
    }
    layout
        .active_node_ids
        .retain(|id| layout.nodes.iter().any(|node| &node.id == id));
    layout
        .orientation_overrides
        .retain(|id, _| layout.nodes.iter().any(|node| &node.id == id));
    layout
        .remote_desktop_origins
        .retain(|id, _| layout.nodes.iter().any(|node| &node.id == id));
    if layout.apply_orientation_overrides() {
        changed = true;
    }
    if changed {
        layout.revision = layout.revision.saturating_add(1);
    }
    orch::set_layout(state.inner(), layout.clone());
    Ok(layout)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn update_settings(
    state: AppStateRef<'_>,
    settings: AppSettings,
) -> Result<AppSettings, CommandError> {
    settings.validate().map_err(CommandError::from)?;
    let previous_shortcuts = state.inner().settings.read().shortcuts.clone();
    if previous_shortcuts != settings.shortcuts {
        crate::shortcuts::apply(
            &state.inner().app,
            &previous_shortcuts,
            &settings.shortcuts,
            Arc::clone(state.inner()),
        )
        .map_err(|message| CommandError::new("invalid_shortcut", message))?;
    }
    let state = Arc::clone(state.inner());
    orch::update_settings(&state, settings).await;
    Ok(orch::settings_snapshot(&state))
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_shortcut_capture(state: AppStateRef<'_>, active: bool) -> Result<(), CommandError> {
    if active {
        crate::shortcuts::suspend(&state.inner().app, state.inner())
            .map_err(|message| CommandError::new("shortcut_capture", message))?;
        crate::orchestrator::pause_remote_for_shortcut_capture(state.inner());
        return Ok(());
    }
    let settings = state.inner().settings.read().shortcuts.clone();
    crate::shortcuts::refresh(&state.inner().app, &settings, Arc::clone(state.inner()))
        .map_err(|message| CommandError::new("shortcut_capture", message))
}

#[tauri::command(rename_all = "snake_case")]
pub fn hide_to_background(state: AppStateRef<'_>) -> Result<(), CommandError> {
    state
        .inner()
        .actions
        .send(UiAction::HideToBackground)
        .map_err(|_| CommandError::internal("编排循环未运行"))?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_files(
    state: AppStateRef<'_>,
    paths: Vec<String>,
    node_id: Option<String>,
) -> Result<(), CommandError> {
    let target = match node_id.as_deref() {
        Some(node_id) => {
            let layout = state.inner().layout.read();
            Some(resolve_drop_target(&layout, node_id)?)
        }
        None => None,
    };
    tracing::info!(files = paths.len(), node = ?node_id, device = ?target, "文件拖放发送请求");
    let state = Arc::clone(state.inner());
    let paths = paths.into_iter().map(std::path::PathBuf::from).collect();
    orch::send_files(&state, paths, target).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn show_file_drop_osd(state: AppStateRef<'_>, file_count: Option<usize>) {
    crate::file_drop::show(state.inner(), file_count);
}

#[tauri::command(rename_all = "snake_case")]
pub fn hide_file_drop_osd(state: AppStateRef<'_>) {
    crate::file_drop::hide(state.inner());
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_file_drop_hover(state: AppStateRef<'_>, node_id: Option<String>) {
    *state.inner().file_drop_hover.lock() = node_id;
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_file_drop_cards(state: AppStateRef<'_>, scale: f64, cards: Vec<FileDropCardView>) {
    *state.inner().file_drop_cards.lock() = Some(FileDropCardsView { scale, cards });
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_input_service_status(state: AppStateRef<'_>) -> InputServiceStatusView {
    let _ = state;
    #[cfg(windows)]
    {
        crate::input_service::status()
    }
    #[cfg(not(windows))]
    {
        InputServiceStatusView {
            installed: false,
            running: false,
            connected: false,
            version: None,
            needs_update: false,
            message: Some("系统输入服务仅支持 Windows".into()),
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn install_input_service(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let _ = state;
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(crate::input_service::install_elevated)
            .await
            .map_err(|error| CommandError::internal(error.to_string()))?
            .map_err(|error| CommandError::new("platform", error))
    }
    #[cfg(not(windows))]
    {
        Err(CommandError::new(
            "unsupported",
            "系统输入服务仅支持 Windows",
        ))
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_input_service(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let _ = state;
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(crate::input_service::uninstall_elevated)
            .await
            .map_err(|error| CommandError::internal(error.to_string()))?
            .map_err(|error| CommandError::new("platform", error))
    }
    #[cfg(not(windows))]
    {
        Err(CommandError::new(
            "unsupported",
            "系统输入服务仅支持 Windows",
        ))
    }
}

fn resolve_drop_target(layout: &Layout, node_id: &str) -> Result<DeviceId, CommandError> {
    let id = DisplayNodeId(node_id.to_string());
    let node = layout
        .node(&id)
        .ok_or_else(|| CommandError::new("invalid_input", "显示节点不存在"))?;
    if !layout.is_active(&id) {
        return Err(CommandError::new("business", "显示节点当前不可用"));
    }
    if node.is_local {
        return Err(CommandError::new("business", "本机屏幕不接收文件传输"));
    }
    Ok(node.device_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_transfer(state: AppStateRef<'_>, id: u64) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::cancel_transfer(&state, id).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn clear_finished_transfers(state: AppStateRef<'_>) -> Result<(), CommandError> {
    orch::clear_finished_transfers(state.inner());
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn open_downloads_folder(state: AppStateRef<'_>) -> Result<(), CommandError> {
    use tauri_plugin_opener::OpenerExt;
    state
        .inner()
        .app
        .opener()
        .open_path(
            state.inner().paths.downloads_dir.display().to_string(),
            None::<&str>,
        )
        .map_err(|error| CommandError::new("platform", error.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub fn mark_notification_read(state: AppStateRef<'_>, id: u64) -> Result<(), CommandError> {
    orch::mark_notification_read(state.inner(), id);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn mark_all_notifications_read(state: AppStateRef<'_>) -> Result<(), CommandError> {
    orch::mark_all_notifications_read(state.inner());
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn clear_notifications(state: AppStateRef<'_>) -> Result<(), CommandError> {
    orch::clear_notifications(state.inner());
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn request_permission(
    state: AppStateRef<'_>,
    kind: String,
) -> Result<bool, CommandError> {
    let kind = parse_permission(&kind)?;
    let state = Arc::clone(state.inner());
    Ok(orch::request_permission(&state, kind).await)
}

#[tauri::command(rename_all = "snake_case")]
pub fn open_permission_settings(state: AppStateRef<'_>, kind: String) -> Result<(), CommandError> {
    let kind = parse_permission(&kind)?;
    crate::permissions::open_settings(&state.inner().app, kind)
        .map_err(|error| CommandError::new("platform", error.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn push_update(
    state: AppStateRef<'_>,
    path: String,
    force: Option<bool>,
    kind: Option<String>,
) -> Result<(), CommandError> {
    let path = std::path::PathBuf::from(path);
    if !path.is_file() {
        return Err(CommandError::new("invalid_input", "更新包不存在"));
    }
    let package_kind = match kind.as_deref() {
        Some("installer") => Some(nc_proto::PackageKind::Installer),
        Some("portable") => Some(nc_proto::PackageKind::Portable),
        Some(other) => {
            return Err(CommandError::new(
                "invalid_input",
                format!("未知更新包类型：{other}"),
            ));
        }
        None => orch::package_kind_for_path(&path),
    };
    let Some(package_kind) = package_kind else {
        return Err(CommandError::new(
            "invalid_input",
            "仅支持 exe 安装包、msi 安装包或便携版 exe",
        ));
    };
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let allowed = match package_kind {
        nc_proto::PackageKind::Installer => {
            (cfg!(windows) && matches!(extension.as_str(), "exe" | "msi"))
                || (cfg!(target_os = "macos") && extension == "pkg")
        }
        nc_proto::PackageKind::Portable => cfg!(windows) && extension == "exe",
    };
    if !allowed {
        return Err(CommandError::new(
            "invalid_input",
            "更新包扩展名与所选类型不匹配",
        ));
    }
    let version = detect_version(&path);
    let state = Arc::clone(state.inner());
    orch::push_update(&state, path, version, force.unwrap_or(false), package_kind).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn apply_update(state: AppStateRef<'_>, version: String) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::apply_staged_update(&state, &version).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rollback_update(state: AppStateRef<'_>, version: String) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::rollback_update(&state, &version).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn retry_connection(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::retry_connection(&state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn show_control_panel(state: AppStateRef<'_>) -> Result<(), CommandError> {
    window::show_control_panel(&state.inner().app)
        .map_err(|error| CommandError::new("platform", error.to_string()))?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn report_surface_ready(
    state: AppStateRef<'_>,
    window: tauri::WebviewWindow,
) -> Result<(), CommandError> {
    let ready = if window.label() == crate::window::STAGE_WINDOW {
        &state.inner().stage_surface_ready
    } else {
        &state.inner().main_surface_ready
    };
    ready.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_control_panel_size(
    state: AppStateRef<'_>,
    expanded: bool,
    scale_factor: f64,
    available_width: f64,
    available_height: f64,
) -> Result<(), CommandError> {
    window::set_control_panel_size(
        &state.inner().app,
        expanded,
        scale_factor,
        available_width,
        available_height,
    )
    .map_err(|error| CommandError::new("platform", error.to_string()))?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn quit_app(state: AppStateRef<'_>) -> Result<(), CommandError> {
    let state = Arc::clone(state.inner());
    orch::shutdown(&state).await;
    state
        .quitting
        .store(true, std::sync::atomic::Ordering::SeqCst);
    state.app.exit(0);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn report_present_stats(
    state: AppStateRef<'_>,
    decode_ms: f64,
    present_ms: f64,
    frames: u32,
    presented_frames: u32,
    dropped_frames: u32,
    timestamp_us: u64,
) -> Result<(), CommandError> {
    orch::report_present_stats(
        state.inner(),
        decode_ms,
        present_ms,
        frames,
        presented_frames,
        dropped_frames,
        timestamp_us,
    );
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn request_keyframe(
    state: AppStateRef<'_>,
    stream_id: Option<u16>,
) -> Result<(), CommandError> {
    let stream_id = StreamId(stream_id.unwrap_or(StreamId::VIDEO.0));
    orch::request_keyframe(state.inner(), stream_id);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn webcodecs_probe_result(
    state: AppStateRef<'_>,
    supported: bool,
    h264: bool,
    reason: Option<String>,
) -> Result<(), CommandError> {
    orch::report_webcodecs(
        state.inner(),
        WebCodecsProbe {
            supported,
            h264,
            reason,
        },
    );
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_autostart(state: AppStateRef<'_>, enabled: bool) -> Result<(), CommandError> {
    #[cfg(windows)]
    crate::autostart::set_enabled(enabled).map_err(|error| CommandError::new("platform", error))?;
    #[cfg(not(windows))]
    {
        use tauri_plugin_autostart::ManagerExt;
        let result = if enabled {
            state.inner().app.autolaunch().enable()
        } else {
            state.inner().app.autolaunch().disable()
        };
        result.map_err(|error| CommandError::new("platform", error.to_string()))?;
    }
    let mut settings = state.inner().settings.read().clone();
    settings.autostart = enabled;
    *state.inner().settings.write() = settings.clone();
    config::save_settings(&state.inner().paths, &settings)
        .map_err(|error| CommandError::new("io", error.to_string()))?;
    Ok(())
}

fn detect_version(path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    for token in name.split(|character: char| !(character.is_ascii_digit() || character == '.')) {
        if let Ok(version) = semver::Version::parse(token) {
            if version.major + version.minor + version.patch > 0 {
                return version.to_string();
            }
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}
