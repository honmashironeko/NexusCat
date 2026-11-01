use crate::actions::UiAction;
use crate::config;
use crate::i18n;
use crate::ipc::{
    self, PairingEventPayload, PairingStage, StageCommand, UpdatePeerState, UpdatePeerView,
    UpdateState,
};
use crate::media_runtime::OutgoingRequest;
use crate::runtime::{
    ActiveControl, AppEvent, AppState, AudioBinding, BrokerEvent, ControlDirection, PairingState,
    PendingRemoteEntry, PendingTakeover, SessionRole, StagePointerGeometry, TransferEntry,
    UpdatePeerPush, UpdatePush,
};
use crate::window;
use nc_clipboard::ClipboardPayload;
use nc_control::{ConnectionPurpose, ControlConfig, ControlEvent, PairingIntent};
use nc_core::{
    AppNotification, AppSettings, DeviceId, DeviceInfo, Direction, DisplayNode, DisplayNodeId,
    InputEnvelope, InputEvent, KeyState, Mode, NotificationAction, NotificationActionKind,
    NotificationCategory, Platform, Point, Rect, SessionSnapshot, Severity, StreamId, UiState,
    VideoTarget,
};
use nc_discovery::{DiscoveredDevice, DiscoveryConfig, DiscoveryEvent};
use nc_files::{FileReceiver, FileSender};
use nc_media::MediaEvent;
use nc_proto::{
    ClipboardContent, ControlMessage, FileTransferId, PackageKind, StreamRequest, UpdateManifest,
};
use nc_session::{SessionEffect, SessionEvent, SessionNotice};
use nc_topology::{EdgeCrossing, Layout};
use nc_update::UpdateReceiver;
use std::net::SocketAddr;
#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;

const FILE_CHUNK_SIZE: usize = 256 * 1024;
const UPDATE_CHUNK_SIZE: usize = 256 * 1024;
const RECONNECT_MAX_ATTEMPTS: u64 = 12;
const PENDING_SESSION_TIMEOUT: Duration = Duration::from_secs(6);
const PENDING_SESSION_MAX_RETRIES: u8 = 1;
const PENDING_TAKEOVER_TIMEOUT: Duration = Duration::from_secs(3);
const CURSOR_EMIT_INTERVAL_MS: u64 = 4;
const ABSOLUTE_EDGE_DWELL_MS: u64 = 100;
const STAGE_EXIT_MOVE_LATCH_MS: u64 = 220;
const EDGE_HALO_TOLERANCE_PX: f64 = 6.0;

pub async fn run(
    state: Arc<AppState>,
    mut actions: UnboundedReceiver<UiAction>,
    mut events: UnboundedReceiver<AppEvent>,
) {
    spawn_periodic_tasks(Arc::clone(&state));
    dispatch_session(&state, SessionEvent::StartDiscovery).await;
    if state.test_mode {
        let _ = window::show_control_panel(&state.app);
        dispatch_session(
            &state,
            SessionEvent::UiStateChanged {
                ui_state: UiState::ControlPanel,
            },
        )
        .await;
    }
    loop {
        tokio::select! {
            Some(action) = actions.recv() => handle_action(&state, action).await,
            Some(event) = events.recv() => handle_event(&state, event).await,
            else => break,
        }
    }
}

fn spawn_periodic_tasks(state: Arc<AppState>) {
    let metrics_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            if metrics_state.events.send(AppEvent::MetricsTick).is_err() {
                break;
            }
        }
    });
    let group_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            if group_state.test_mode
                || group_state.user_disconnect.load(Ordering::SeqCst)
                || group_state.quitting.load(Ordering::SeqCst)
            {
                continue;
            }
            let local = group_state.identity.device_id();
            let peers: Vec<DeviceId> = group_state.devices.read().keys().copied().collect();
            for peer in peers {
                if !should_initiate_auto_connect(local, peer) {
                    continue;
                }
                let trusted = group_state
                    .trust
                    .read()
                    .map(|store| store.get(&peer).is_some())
                    .unwrap_or(false);
                if trusted {
                    let state_clone = Arc::clone(&group_state);
                    tokio::spawn(async move {
                        ensure_group_link(&state_clone, peer, PairingIntent::Trusted, false).await;
                    });
                }
            }
        }
    });
    let permission_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        loop {
            ticker.tick().await;
            if permission_state
                .events
                .send(AppEvent::PermissionTick)
                .is_err()
            {
                break;
            }
        }
    });
    let secure_desktop_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        loop {
            ticker.tick().await;
            if secure_desktop_state
                .events
                .send(AppEvent::SecureDesktopTick)
                .is_err()
            {
                break;
            }
        }
    });
    let edge_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(20));
        loop {
            ticker.tick().await;
            let snapshot = edge_state.snapshot();
            let role = *edge_state.session_role.read();
            let released = edge_state.input.lock().remote_input_released;
            clear_released_host_lock(&edge_state, role, released);
            if edge_state.secure_desktop.load(Ordering::SeqCst)
                || !local_flow_allowed(&edge_state, &snapshot, role, released)
            {
                edge_state.input.lock().edge_detector.clear_pending();
                crate::osd::hide_halo(&edge_state);
                continue;
            }
            if edge_state.events.send(AppEvent::EdgeTick).is_err() {
                break;
            }
        }
    });
    let clipboard_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        loop {
            ticker.tick().await;
            if clipboard_state
                .events
                .send(AppEvent::ClipboardTick)
                .is_err()
            {
                break;
            }
        }
    });
}

async fn handle_event(state: &Arc<AppState>, event: AppEvent) {
    match event {
        AppEvent::Action(action) => handle_action(state, action).await,
        AppEvent::Discovery(event) => handle_discovery(state, event).await,
        AppEvent::Control {
            connection_id,
            event,
        } => handle_control_event(state, connection_id, event).await,
        AppEvent::Session(event) => dispatch_session(state, event).await,
        AppEvent::Input(captured) => handle_input(state, captured).await,
        AppEvent::Media {
            connection_id,
            event,
        } => handle_media_event(state, connection_id, event).await,
        AppEvent::MetricsTick => {
            emit_metrics(state);
            if let Ok(monitors) = nc_platform::monitors() {
                let changed = *state.monitors.read() != monitors;
                *state.monitors.write() = monitors;
                if changed {
                    reconcile_local_layout(state);
                }
            }
            check_input_watchdog(state).await;
            check_pending_watchdog(state).await;
            maybe_adapt(state);
            if state.input.lock().controlling_remote {
                emit_remote_cursor(state);
            }
        }
        AppEvent::FileDropBroker(event) => handle_file_drop_broker_event(state, event),
        AppEvent::SecureDesktopTick => check_secure_desktop(state).await,
        AppEvent::PermissionTick => check_permissions(state).await,
        AppEvent::WebCodecs(probe) => {
            let supported = probe.supported && probe.h264;
            *state.webcodecs.write() = probe;
            if !supported {
                let (title, body) = i18n::webcodecs_unsupported(state.language());
                add_notification(
                    state,
                    Severity::Warning,
                    NotificationCategory::Media,
                    title,
                    body,
                    None,
                );
            }
        }
        AppEvent::PresentStats {
            decode_ms,
            present_ms,
            frames,
            presented_frames,
            dropped_frames,
            timestamp_us,
        } => {
            static FIRST_PRESENT_STATS: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !FIRST_PRESENT_STATS.swap(true, Ordering::Relaxed) {
                tracing::info!(
                    decode_ms,
                    present_ms,
                    frames,
                    presented_frames,
                    dropped_frames,
                    timestamp_us,
                    "收到前端呈现统计"
                );
            }
            state.metrics.record_received_frames(
                frames,
                (decode_ms.max(0.0) * 1000.0) as u64,
                (present_ms.max(0.0) * 1000.0) as u64,
                dropped_frames,
            );
            state
                .metrics
                .record_presented_frames(presented_frames);
            let now_ms = crate::notifications::now_ms();
            let clock_offset_us = state
                .session_peer_id()
                .and_then(|device| state.clock_offset(device))
                .unwrap_or(0);
            match estimate_glass_to_glass_ms(now_ms, timestamp_us, clock_offset_us) {
                Some(glass_ms) => state.metrics.record_glass_to_glass(glass_ms),
                None => {
                    static REJECTED: AtomicU64 = AtomicU64::new(0);
                    if REJECTED.fetch_add(1, Ordering::Relaxed) % 10 == 0 {
                        let raw_delta_ms =
                            now_ms as i64 - (timestamp_us / 1000) as i64;
                        let corrected_delta_ms = now_ms as i64
                            - ((timestamp_us as i64 + clock_offset_us) / 1000);
                        tracing::info!(
                            now_ms,
                            timestamp_us,
                            clock_offset_us,
                            raw_delta_ms,
                            corrected_delta_ms,
                            "端到端估算被丢弃（时钟不同步或帧过旧）"
                        );
                    }
                }
            }
        }
        AppEvent::ReconnectWake {
            generation,
            attempt,
            max,
        } => {
            if generation != state.generation() {
                return;
            }
            dispatch_session(state, SessionEvent::ReconnectTick { attempt, max }).await;
        }
        AppEvent::TransferTick { id } => advance_transfer(state, id).await,
        AppEvent::FileDropHint => notify_file_drop_hint(state),
        AppEvent::EdgeTick => check_edge_crossing_tick(state).await,
        AppEvent::ClipboardTick => poll_clipboard(state).await,
        AppEvent::Clipboard(payload) => {
            let content = match payload {
                ClipboardPayload::Text(text) => ClipboardContent::Text(text),
                ClipboardPayload::ImagePng(bytes) => ClipboardContent::ImagePng(bytes),
            };
            if !send_control(state, ControlMessage::Clipboard { content }) {
                let mut hash = state.clipboard_last_hash.write();
                *hash = None;
            }
        }
        AppEvent::ClipboardError(error) => {
            tracing::debug!(%error, "剪贴板轮询失败");
        }
    }
}

async fn handle_action(state: &Arc<AppState>, action: UiAction) {
    match action {
        UiAction::ShowControlPanel => {
            open_control_layer(state, false).await;
            ipc::emit(&state.app, ipc::EVENT_OPEN_WORKBENCH, ());
        }
        UiAction::OpenNotifications => {
            open_control_layer(state, false).await;
        }
        UiAction::ShowCalibration => {
            open_control_layer(state, true).await;
        }
        UiAction::ShowSettings => {
            open_control_layer(state, false).await;
            ipc::emit(&state.app, ipc::EVENT_OPEN_SETTINGS, ());
        }
        UiAction::MarkAllNotificationsRead => {
            mark_all_notifications_read(state);
        }
        UiAction::NotificationAction { kind, payload } => {
            mark_all_notifications_read(state);
            match kind {
                nc_core::NotificationActionKind::OpenControlPanel => {
                    open_control_layer(state, false).await;
                    ipc::emit(&state.app, ipc::EVENT_OPEN_WORKBENCH, ());
                }
                nc_core::NotificationActionKind::OpenSettings => {
                    open_control_layer(state, false).await;
                    ipc::emit(&state.app, ipc::EVENT_OPEN_SETTINGS, ());
                }
                nc_core::NotificationActionKind::ShowStage => {
                    summon_stage(state).await;
                }
                nc_core::NotificationActionKind::RetryConnection => {
                    if let Some(payload) = payload.as_deref() {
                        if let Ok(device) = DeviceId::from_hex(payload) {
                            if let Err(error) = begin_connect(state, device).await {
                                tracing::warn!(%error, "通知动作重试连接失败");
                            }
                        }
                    }
                }
                nc_core::NotificationActionKind::OpenUpdate => {
                    open_control_layer(state, false).await;
                    ipc::emit(&state.app, ipc::EVENT_OPEN_SETTINGS, ());
                    ipc::emit(&state.app, ipc::EVENT_OPEN_UPDATE, ());
                }
                nc_core::NotificationActionKind::Dismiss => {}
            }
        }
        UiAction::ToggleEdge => {
            let mut next = state.settings.read().clone();
            next.edge_enabled = !next.edge_enabled;
            update_settings(state, next).await;
        }
        UiAction::HideToBackground => {
            dispatch_session(
                state,
                SessionEvent::UiStateChanged {
                    ui_state: UiState::Background,
                },
            )
            .await;
            let _ = window::hide_to_background(&state.app);
        }
        UiAction::SummonStage => dispatch_session(state, SessionEvent::StageSummon).await,
        UiAction::HideStage => dispatch_session(state, SessionEvent::StageHide).await,
        UiAction::ToggleStage => toggle_stage(state).await,
        UiAction::ToggleRemotePause => toggle_remote_pause(state).await,
        UiAction::SetMode(mode) => dispatch_session(state, SessionEvent::SetMode { mode }).await,
        UiAction::ToggleMode => dispatch_session(state, SessionEvent::ToggleMode).await,
        UiAction::StartDiscovery => dispatch_session(state, SessionEvent::StartDiscovery).await,
        UiAction::StopDiscovery => dispatch_session(state, SessionEvent::StopDiscovery).await,
        UiAction::PairDevice(device) => begin_pairing(state, device),
        UiAction::ConnectDevice(device) => {
            if let Err(error) = begin_connect(state, device).await {
                tracing::warn!(%error, "连接设备失败");
                let language = state.language();
                let (title, body) = i18n::connection_failed(language, &error.to_string());
                add_notification(
                    state,
                    Severity::Warning,
                    NotificationCategory::Connection,
                    title,
                    body,
                    None,
                );
            }
        }
        UiAction::Disconnect => {
            state.user_disconnect.store(true, Ordering::SeqCst);
            state.reconnect_attempt.store(0, Ordering::SeqCst);
            if let Some(handle) = state.reconnect_task.lock().take() {
                handle.abort();
            }
            dispatch_session(state, SessionEvent::UserAbort).await;
            cleanup_connection(state).await;
        }
        UiAction::Quit => {
            state.quitting.store(true, Ordering::SeqCst);
            dispatch_session(state, SessionEvent::Shutdown).await;
            cleanup_connection(state).await;
            let _ = tokio::task::spawn_blocking(|| {}).await;
            state.app.exit(0);
        }
    }
}

async fn open_control_layer(state: &Arc<AppState>, calibration: bool) {
    crate::osd::hide_halo(state);
    dispatch_session(
        state,
        SessionEvent::UiStateChanged {
            ui_state: UiState::ControlPanel,
        },
    )
    .await;
    schedule_control_panel_show(state);
    if calibration {
        ipc::emit(&state.app, ipc::EVENT_CALIBRATION_OPEN, ());
    }
}

fn schedule_control_panel_show(state: &Arc<AppState>) {
    let app = state.app.clone();
    let state = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        if state.snapshot().ui_state != UiState::ControlPanel {
            return;
        }
        let visible = window::main_window(&app)
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false);
        let pending = state
            .main_pending_surface_render
            .swap(false, Ordering::SeqCst);
        if visible && pending {
            wait_for_surface_ready(&state, false, 400).await;
        } else if !visible {
            let _ = window::show_main(&app);
            wait_for_surface_ready(&state, false, 300).await;
        }
        if state.snapshot().ui_state == UiState::ControlPanel {
            let _ = window::show_control_panel(&app);
        }
    });
}

async fn wait_for_surface_ready(state: &AppState, stage: bool, timeout_ms: u64) {
    let ready = if stage {
        &state.stage_surface_ready
    } else {
        &state.main_surface_ready
    };
    let started = Instant::now();
    while !ready.load(Ordering::SeqCst) && started.elapsed() < Duration::from_millis(timeout_ms) {
        tokio::time::sleep(Duration::from_millis(8)).await;
    }
}

async fn dispatch_session(state: &Arc<AppState>, event: SessionEvent) {
    let (effects, snapshot) = {
        let mut machine = state.session.lock();
        let effects = machine.handle_at(event, Some(crate::notifications::now_ms()));
        (effects, machine.snapshot())
    };
    let previous_ui_state = state.snapshot.read().ui_state;
    *state.snapshot.write() = snapshot.clone();
    if snapshot.ui_state != UiState::Stage {
        set_switching_target(state, false);
    }
    if previous_ui_state != snapshot.ui_state {
        if snapshot.ui_state == UiState::Stage {
            state.stage_surface_ready.store(false, Ordering::SeqCst);
            state
                .stage_pending_surface_render
                .store(true, Ordering::SeqCst);
        } else {
            state.main_surface_ready.store(false, Ordering::SeqCst);
            state
                .main_pending_surface_render
                .store(true, Ordering::SeqCst);
        }
    }
    state
        .input
        .lock()
        .active
        .store(snapshot.state.is_active(), Ordering::SeqCst);
    ipc::emit(&state.app, ipc::EVENT_SESSION_CHANGED, snapshot.clone());
    update_tray(state, &snapshot);
    apply_effects(state, effects).await;
}

#[derive(Clone, Copy)]
struct StageMonitor {
    position: tauri::PhysicalPosition<f64>,
    size: tauri::PhysicalSize<u32>,
}

fn stage_monitor_position(
    state: &AppState,
    window: &tauri::WebviewWindow,
) -> Option<StageMonitor> {
    if state.settings.read().stage_on_primary {
        let monitor = window.primary_monitor().ok().flatten()?;
        let position = monitor.position();
        let size = monitor.size();
        return Some(StageMonitor {
            position: tauri::PhysicalPosition::new(position.x as f64, position.y as f64),
            size: tauri::PhysicalSize::new(size.width, size.height),
        });
    }
    #[cfg(windows)]
    if let Some(monitor) = stage_entry_monitor_position(state) {
        return Some(monitor);
    }
    let monitor = nc_platform::cursor_position()
        .ok()
        .and_then(|(x, y)| window.monitor_from_point(x as f64, y as f64).ok().flatten())
        .or_else(|| window.current_monitor().ok().flatten())?;
    let position = monitor.position();
    let size = monitor.size();
    Some(StageMonitor {
        position: tauri::PhysicalPosition::new(position.x as f64, position.y as f64),
        size: tauri::PhysicalSize::new(size.width, size.height),
    })
}

#[cfg(windows)]
fn stage_entry_monitor_position(state: &AppState) -> Option<StageMonitor> {
    if state.snapshot().input_owner.is_local() {
        return None;
    }
    let node_id = state.input.lock().local_entry_node.clone()?;
    let node = {
        let layout = state.layout.read();
        layout.node(&node_id).cloned()?
    };
    if !node.is_local {
        return None;
    }
    let monitors = state.monitors.read();
    let monitor = monitor_for_local_node(&node, &monitors)?;
    Some(StageMonitor {
        position: tauri::PhysicalPosition::new(
            monitor.bounds.x as f64,
            monitor.bounds.y as f64,
        ),
        size: tauri::PhysicalSize::new(monitor.bounds.width, monitor.bounds.height),
    })
}

fn schedule_stage_show(state: &Arc<AppState>) {
    let app = state.app.clone();
    let state = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        if !state.snapshot().stage_visible {
            return;
        }
        let (stage, created) = match window::ensure_stage_window(&app) {
            Ok(pair) => pair,
            Err(error) => {
                tracing::warn!(%error, "创建全屏窗口失败");
                return;
            }
        };
        if created {
            let actions = state.actions.clone();
            window::install_stage_close_handler(&stage, move || {
                let _ = actions.send(crate::actions::UiAction::HideStage);
            });
        }
        let pending = state
            .stage_pending_surface_render
            .swap(false, Ordering::SeqCst);
        let visible = stage.is_visible().unwrap_or(false);
        if visible && pending {
            wait_for_surface_ready(&state, true, 500).await;
        }
        if !state.snapshot().stage_visible {
            return;
        }
        let target = stage_monitor_position(&state, &stage);
        let current = stage.current_monitor().ok().flatten().map(|monitor| {
            let position = monitor.position();
            (position.x, position.y)
        });
        let already_fullscreen = stage.is_fullscreen().unwrap_or(false);
        let same_monitor = target.is_some_and(|target| {
            current
                == Some((
                    target.position.x.round() as i32,
                    target.position.y.round() as i32,
                ))
        });
        if !(already_fullscreen && same_monitor) {
            if already_fullscreen {
                let _ = stage.set_fullscreen(false);
            }
            if let Some(target) = target {
                let _ = stage.set_position(target.position);
                let _ = stage.set_size(target.size);
            }
        }
        let _ = window::show_stage(&app);
        if let Some(target) = target {
            if !(already_fullscreen && same_monitor) {
                let _ = stage.set_fullscreen_on_monitor(target.position);
            }
        } else if !already_fullscreen {
            let _ = stage.set_fullscreen(true);
        }
        let _ = window::hide_main(&app);
        let _ = stage.set_focus();
    });
}

async fn apply_effects(state: &Arc<AppState>, effects: Vec<SessionEffect>) {
    let mut stage_input_scheduled = false;
    let mut stage_show_scheduled = false;
    for effect in effects {
        match effect {
            SessionEffect::StartDiscovery => {
                start_discovery(state);
            }
            SessionEffect::StopDiscovery => {
                if let Some(service) = state.discovery.lock().take() {
                    service.stop();
                }
            }
            SessionEffect::Connect => {
                let pending = *state.pending_connect.read();
                if let Some(device) = pending {
                    start_outgoing_connection(state, device, PairingIntent::Trusted).await;
                }
            }
            SessionEffect::StartPairing => {
                let pending = *state.pending_connect.read();
                if let Some(device) = pending {
                    start_outgoing_connection(state, device, PairingIntent::Pair).await;
                }
            }
            SessionEffect::Disconnect => cleanup_connection(state).await,
            SessionEffect::StartMedia { target } => {
                if let Some(target) = target {
                    start_media(state, target).await;
                }
            }
            SessionEffect::StopMedia => stop_media(state).await,
            SessionEffect::ShowControlPanel => {
                schedule_control_panel_show(state);
            }
            SessionEffect::ShowStage => {
                crate::osd::hide_halo(state);
                state.media.request_all_keyframes();
                send_control(
                    state,
                    ControlMessage::KeyframeRequest {
                        stream_id: StreamId::VIDEO,
                    },
                );
                ipc::emit(
                    &state.app,
                    ipc::EVENT_STAGE_COMMAND,
                    StageCommand { action: "show" },
                );
                if !stage_show_scheduled {
                    stage_show_scheduled = true;
                    schedule_stage_show(state);
                }
                if !stage_input_scheduled {
                    stage_input_scheduled = true;
                    let state_clone = Arc::clone(state);
                    tauri::async_runtime::spawn(async move {
                        ensure_remote_stage_input(&state_clone).await;
                    });
                }
                emit_remote_cursor(state);
            }
            SessionEffect::HideStage => {
                reset_input_runtime(state);
                state.input.lock().local_entry_node = None;
            }
            SessionEffect::BackToBackground => {
                reset_input_runtime(state);
                state.input.lock().local_entry_node = None;
                let _ = window::hide_to_background(&state.app);
            }
            SessionEffect::SwitchWindowState(ui_state) => match ui_state {
                UiState::Background => {
                    reset_input_runtime(state);
                    state.input.lock().local_entry_node = None;
                    let _ = window::hide_to_background(&state.app);
                }
                UiState::ControlPanel => {
                    reset_input_runtime(state);
                    state.input.lock().local_entry_node = None;
                    schedule_control_panel_show(state);
                }
                UiState::Stage => {
                    if !stage_show_scheduled {
                        stage_show_scheduled = true;
                        schedule_stage_show(state);
                    }
                    if !stage_input_scheduled {
                        stage_input_scheduled = true;
                        let state_clone = Arc::clone(state);
                        tauri::async_runtime::spawn(async move {
                            ensure_remote_stage_input(&state_clone).await;
                        });
                    }
                }
            },
            SessionEffect::ReleaseInput => {
                let was_controlling = state.input.lock().controlling_remote;
                let was_host = *state.session_role.read() == Some(SessionRole::Host);
                let switching = state.pending_session.read().is_some();
                reset_input_runtime(state);
                if was_controlling {
                    tracing::info!(was_host, switching, "会话效果释放输入：发送 ReleaseAll");
                    send_control(state, ControlMessage::ReleaseAll);
                }
                if was_host && !switching {
                    broadcast_host_status(state, false);
                }
            }
            SessionEffect::GrantInput(device) => {
                let local = state.identity.device_id();
                let host = *state.session_role.read() == Some(SessionRole::Host);
                let mut input = state.input.lock();
                let suppress = device != local && input.controlling_remote;
                input.set_suppressed(suppress);
                if device == local {
                    input.set_relative_mode(false);
                    input.end_remote_cursor();
                    input.control_started_ms = 0;
                    input.edge_detector.clear_pending();
                    input.release_all();
                }
                drop(input);
                if host && device != local {
                    broadcast_host_status(state, true);
                }
            }
            SessionEffect::PersistMode(mode) => {
                let mut settings = state.settings.read().clone();
                settings.mode = mode;
                *state.settings.write() = settings.clone();
                if let Err(error) = config::save_settings(&state.paths, &settings) {
                    tracing::warn!(%error, "保存模式设置失败");
                }
                if *state.session_role.read() == Some(SessionRole::Host) {
                    send_control(state, ControlMessage::ModeChange { mode });
                }
            }
            SessionEffect::Notice(notice) => handle_notice(state, notice),
        }
    }
}

fn handle_notice(state: &AppState, notice: SessionNotice) {
    let language = state.language();
    let (title, body) = match notice {
        SessionNotice::Paired => i18n::paired(
            language,
            &state
                .peer()
                .map(|peer| peer.name)
                .unwrap_or_else(|| i18n::generic_device(language)),
        ),
        SessionNotice::PairingFailed => {
            let reason = state
                .snapshot()
                .last_error
                .unwrap_or_else(|| i18n::generic_pairing_failed(language));
            i18n::pairing_failed(language, &reason)
        }
        SessionNotice::Connected => i18n::connected(
            language,
            &state
                .peer()
                .map(|peer| peer.name)
                .unwrap_or_else(|| i18n::generic_device(language)),
        ),
        SessionNotice::Disconnected => i18n::disconnected(
            language,
            &state
                .snapshot()
                .last_error
                .unwrap_or_else(|| i18n::generic_disconnected(language)),
        ),
        SessionNotice::Reconnecting => i18n::reconnecting(
            language,
            state.reconnect_attempt.load(Ordering::SeqCst) as u32 + 1,
        ),
        SessionNotice::ReconnectFailed => {
            let reason = state
                .snapshot()
                .last_error
                .unwrap_or_else(|| i18n::generic_reconnect_failed(language));
            i18n::reconnect_failed(language, &reason)
        }
        SessionNotice::ModeChanged => i18n::mode_changed(language, state.snapshot().mode),
        SessionNotice::PermissionLost => {
            let kind = state
                .permissions
                .read()
                .statuses
                .iter()
                .find(|status| !status.granted)
                .map(|status| status.kind)
                .unwrap_or(nc_core::PermissionKind::ScreenRecording);
            i18n::permission_lost(language, kind)
        }
        SessionNotice::EmergencyRelease => i18n::emergency_release(language),
        SessionNotice::ControlHandover => i18n::control_handover(
            language,
            &state
                .peer()
                .map(|peer| peer.name)
                .unwrap_or_else(|| i18n::generic_remote_device(language)),
        ),
        SessionNotice::ControlReturned => i18n::control_returned(language),
    };
    let severity = match notice {
        SessionNotice::PairingFailed
        | SessionNotice::ReconnectFailed
        | SessionNotice::PermissionLost => Severity::Error,
        SessionNotice::Disconnected
        | SessionNotice::Reconnecting
        | SessionNotice::ControlHandover => Severity::Warning,
        _ => Severity::Info,
    };
    let category = match notice {
        SessionNotice::Paired | SessionNotice::PairingFailed => NotificationCategory::Pairing,
        SessionNotice::Connected
        | SessionNotice::Disconnected
        | SessionNotice::Reconnecting
        | SessionNotice::ReconnectFailed => NotificationCategory::Connection,
        SessionNotice::PermissionLost => NotificationCategory::Permission,
        SessionNotice::ControlHandover | SessionNotice::ControlReturned => {
            NotificationCategory::Connection
        }
        _ => NotificationCategory::System,
    };
    if matches!(
        notice,
        SessionNotice::ControlHandover | SessionNotice::ControlReturned
    ) {
        add_priority_notification(state, severity, category, title, body, None);
    } else {
        add_notification(state, severity, category, title, body, None);
    }
}

fn start_discovery(state: &Arc<AppState>) {
    if state.discovery.lock().is_some() {
        return;
    }
    let settings = state.settings.read().clone();
    let mut config = DiscoveryConfig {
        discovery_port: settings.discovery_port,
        ..DiscoveryConfig::default()
    };
    if state.test_mode {
        if let Ok(port) = std::env::var("NEXUSCAT_DISCOVERY_PORT") {
            if let Ok(port) = port.parse::<u16>() {
                config.discovery_port = port;
            }
        }
        if let Ok(peers) = std::env::var("NEXUSCAT_DISCOVERY_PEERS") {
            config.extra_targets = peers
                .split(',')
                .filter_map(|peer| peer.trim().parse::<SocketAddr>().ok())
                .collect();
            config.interface_broadcast = config.extra_targets.is_empty();
        }
    }
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(64);
    let local_info = state.device_info.read().clone();
    let service = match nc_discovery::DiscoveryService::start_with_device_info(
        config, local_info, event_tx,
    ) {
        Ok(service) => service,
        Err(error) => {
            tracing::warn!(%error, "启动设备发现失败");
            let language = state.language();
            let (title, body) = i18n::discovery_failed(language, &error.to_string());
            add_notification(
                state,
                Severity::Error,
                NotificationCategory::Connection,
                title,
                body,
                None,
            );
            return;
        }
    };
    *state.discovery.lock() = Some(service);
    let forward_state = Arc::clone(state);
    tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    if forward_state
                        .events
                        .send(AppEvent::Discovery(event))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn should_auto_connect(
    test_mode: bool,
    user_disconnect: bool,
    trusted: bool,
    already_connected: bool,
    connecting: bool,
) -> bool {
    !test_mode && !user_disconnect && trusted && !already_connected && !connecting
}

fn should_initiate_auto_connect(local: DeviceId, peer: DeviceId) -> bool {
    local < peer
}

async fn maybe_auto_connect(state: &Arc<AppState>, peer: DeviceId) {
    if !should_initiate_auto_connect(state.identity.device_id(), peer) {
        return;
    }
    let trusted = state
        .trust
        .read()
        .map(|store| store.get(&peer).is_some())
        .unwrap_or(false);
    let already_connected = state
        .links
        .lock()
        .get(&peer)
        .is_some_and(|link| link.connected.load(Ordering::SeqCst));
    let connecting = state.connecting_links.lock().contains(&peer);
    if !should_auto_connect(
        state.test_mode,
        state.user_disconnect.load(Ordering::SeqCst),
        trusted,
        already_connected,
        connecting,
    ) {
        return;
    }

    ensure_group_link(state, peer, PairingIntent::Trusted, false).await;
}

async fn handle_discovery(state: &Arc<AppState>, event: DiscoveryEvent) {
    match event {
        DiscoveryEvent::Found(device) | DiscoveryEvent::Updated(device) => {
            let peer = device.info.id;
            tracing::info!(
                device_id = %peer,
                name = %device.info.name,
                address = %device.address,
                online = true,
                "发现设备更新"
            );
            state.devices.write().insert(peer, device);
            maybe_auto_connect(state, peer).await;
        }
        DiscoveryEvent::Lost(device) => {
            tracing::info!(device_id = %device, "设备离线");
            state.devices.write().remove(&device);
            let still_linked = state
                .links
                .lock()
                .get(&device)
                .is_some_and(|link| link.connected.load(Ordering::SeqCst));
            if !still_linked {
                deactivate_device_nodes(state, device);
                if *state.pending_session.read() == Some(device) {
                    abort_pending_session(state, "目标设备离线", false).await;
                    let language = state.language();
                    let (title, body) = i18n::device_offline(language);
                    add_notification(
                        state,
                        Severity::Warning,
                        NotificationCategory::Connection,
                        title,
                        body,
                        Some(NotificationAction {
                            kind: NotificationActionKind::RetryConnection,
                            label: i18n::retry_action(language),
                            payload: Some(device.to_hex()),
                        }),
                    );
                }
                let takeover = state
                    .pending_takeover
                    .lock()
                    .as_ref()
                    .map(|request| (request.host, request.target));
                if let Some((host, target)) = takeover {
                    if target == device {
                        clear_pending_takeover(state);
                        let language = state.language();
                        let (title, body) = i18n::device_offline(language);
                        add_notification(
                            state,
                            Severity::Warning,
                            NotificationCategory::Connection,
                            title,
                            body,
                            Some(NotificationAction {
                                kind: NotificationActionKind::RetryConnection,
                                label: i18n::retry_action(language),
                                payload: Some(target.to_hex()),
                            }),
                        );
                    } else if host == device {
                        clear_pending_takeover(state);
                        if *state.group_host.read() == Some(host) {
                            set_group_host(state, None);
                        }
                        let language = state.language();
                        let host_name = device_display_name(state, host);
                        let (title, body) = i18n::host_offline(language, &host_name);
                        add_notification(
                            state,
                            Severity::Warning,
                            NotificationCategory::Connection,
                            title,
                            body,
                            Some(NotificationAction {
                                kind: NotificationActionKind::RetryConnection,
                                label: i18n::retry_action(language),
                                payload: Some(target.to_hex()),
                            }),
                        );
                    }
                }
                if *state.group_host.read() == Some(device) {
                    set_group_host(state, None);
                }
            }
        }
    }
    emit_devices(state);
    dispatch_session(state, SessionEvent::DevicesUpdated).await;
}

fn emit_devices(state: &AppState) {
    ipc::emit(&state.app, ipc::EVENT_DEVICES_CHANGED, state.device_views());
}

pub fn group_status(state: &AppState) -> ipc::GroupStatusView {
    ipc::GroupStatusView {
        active_host: *state.group_host.read(),
        pending_target: *state.pending_session.read(),
        takeover_target: state
            .pending_takeover
            .lock()
            .as_ref()
            .map(|pending| pending.target),
    }
}

fn emit_group_status(state: &AppState) {
    ipc::emit(&state.app, ipc::EVENT_GROUP_STATUS, group_status(state));
}

fn set_group_host(state: &AppState, next: Option<DeviceId>) {
    let changed = {
        let mut host = state.group_host.write();
        if *host == next {
            false
        } else {
            *host = next;
            true
        }
    };
    if changed {
        emit_group_status(state);
    }
}

fn set_pending_session(state: &AppState, target: Option<DeviceId>) {
    let changed = {
        let mut pending = state.pending_session.write();
        if *pending == target {
            false
        } else {
            *pending = target;
            true
        }
    };
    if changed {
        *state.pending_session_started.lock() = target.map(|_| Instant::now());
        state.pending_session_retries.store(0, Ordering::SeqCst);
        emit_group_status(state);
    }
}

fn restart_pending_session_timer(state: &AppState) {
    if state.pending_session.read().is_some() {
        *state.pending_session_started.lock() = Some(Instant::now());
        state.pending_session_retries.store(0, Ordering::SeqCst);
    }
}

fn clear_pending_takeover(state: &AppState) {
    let cleared = state.pending_takeover.lock().take().is_some();
    if cleared {
        emit_group_status(state);
    }
}

fn target_is_available(state: &AppState, device: DeviceId) -> bool {
    state.devices.read().contains_key(&device)
        || state
            .links
            .lock()
            .get(&device)
            .is_some_and(|link| link.connected.load(Ordering::SeqCst))
}

fn control_config(state: &AppState) -> ControlConfig {
    ControlConfig {
        identity: Arc::clone(&state.identity),
        certificate: Arc::clone(&state.certificate),
        trust_store: Arc::clone(&state.trust),
        trust_path: Some(state.paths.trust_file.clone()),
        connect_timeout: Duration::from_secs(5),
        handshake_timeout: Duration::from_secs(60),
        heartbeat_interval: Duration::from_secs(2),
        heartbeat_timeout: Duration::from_secs(6),
        local_device: state.device_info.read().clone(),
    }
}

async fn start_outgoing_connection(
    state: &Arc<AppState>,
    device_id: DeviceId,
    intent: PairingIntent,
) {
    ensure_group_link(state, device_id, intent, true).await;
}

async fn ensure_group_link(
    state: &Arc<AppState>,
    device_id: DeviceId,
    intent: PairingIntent,
    requested: bool,
) {
    if !requested && state.user_disconnect.load(Ordering::SeqCst) {
        return;
    }
    if state.links.lock().contains_key(&device_id) {
        if *state.pending_session.read() == Some(device_id) {
            send_session_open(state, device_id);
        }
        return;
    }
    if !state.connecting_links.lock().insert(device_id) {
        return;
    }

    let language = state.language();
    let discovered = state.devices.read().get(&device_id).cloned();
    let Some(discovered) = discovered else {
        state.connecting_links.lock().remove(&device_id);
        if requested {
            let (title, body) = i18n::device_offline(language);
            add_notification(
                state,
                Severity::Warning,
                NotificationCategory::Connection,
                title,
                body,
                None,
            );
            fail_pending_session(state, device_id, "设备不在线".into(), intent);
        }
        return;
    };
    if discovered.info.control_port == 0 {
        state.connecting_links.lock().remove(&device_id);
        if requested {
            let (title, body) = i18n::device_port_not_ready(language);
            add_notification(
                state,
                Severity::Warning,
                NotificationCategory::Connection,
                title,
                body,
                None,
            );
            fail_pending_session(state, device_id, "设备端口未就绪".into(), intent);
        }
        return;
    }
    let addr = SocketAddr::new(discovered.address.ip(), discovered.info.control_port);
    let peer_name = discovered.info.name.clone();
    tracing::info!(%device_id, %addr, ?intent, "发起设备组连接");
    let config = control_config(state);
    let events = state.events.clone();
    let state_clone = Arc::clone(state);
    tokio::spawn(async move {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
        match nc_control::ControlConnection::connect(
            config,
            addr,
            intent,
            ConnectionPurpose::Group,
            event_tx,
        )
        .await
        {
            Ok(connection) => {
                tracing::info!(%addr, "设备组连接握手完成");
                let id = state_clone.next_generation();
                let link = ActiveControl::new(id, connection, ControlDirection::Outgoing);
                let replaced = state_clone.links.lock().insert(device_id, link);
                if let Some(replaced) = replaced {
                    replaced.connection.close();
                }
                state_clone.connecting_links.lock().remove(&device_id);
                if intent == PairingIntent::Pair {
                    *state_clone.pairing.lock() = Some(PairingState {
                        device_id: Some(device_id),
                        device_name: Some(peer_name),
                        sas: None,
                    });
                }
                while let Some(event) = event_rx.recv().await {
                    if events
                        .send(AppEvent::Control {
                            connection_id: id,
                            event,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "建立设备组连接失败");
                state_clone.connecting_links.lock().remove(&device_id);
                let reason = error.to_string();
                let snapshot = state_clone.snapshot();
                let retry_after_failure = snapshot.state.is_active()
                    || snapshot.state == nc_core::SessionState::Reconnecting;
                if requested {
                    fail_pending_session(&state_clone, device_id, reason, intent);
                }
                if requested && intent == PairingIntent::Trusted && retry_after_failure {
                    set_pending_session(&state_clone, Some(device_id));
                    let peer = state_clone
                        .devices
                        .read()
                        .get(&device_id)
                        .map(|device| device.info.clone());
                    schedule_reconnect(&state_clone, peer).await;
                }
            }
        }
    });
}

fn fail_pending_session(
    state: &AppState,
    device_id: DeviceId,
    reason: String,
    intent: PairingIntent,
) {
    if *state.session_role.read() == Some(SessionRole::Host) {
        broadcast_host_status(state, false);
    }
    if *state.pending_connect.read() == Some(device_id) {
        *state.pending_connect.write() = None;
    }
    if *state.pending_session.read() == Some(device_id) {
        set_pending_session(state, None);
        set_switching_target(state, false);
        reset_input_runtime(state);
    }
    if state
        .pending_remote_entry
        .lock()
        .as_ref()
        .is_some_and(|pending| pending.device == device_id)
    {
        *state.pending_remote_entry.lock() = None;
    }
    let session_event = if intent == PairingIntent::Pair {
        SessionEvent::PairingFailed {
            reason: reason.clone(),
        }
    } else {
        SessionEvent::ConnectionLost {
            reason: reason.clone(),
        }
    };
    let _ = state.events.send(AppEvent::Session(session_event));
    if intent == PairingIntent::Pair {
        emit_pairing_failed(state, Some(device_id), reason);
    }
}

fn send_session_open(state: &AppState, device_id: DeviceId) {
    let Some(peer) = state.link_peer(device_id) else {
        return;
    };
    let settings = state.settings.read().clone();
    let target = target_for_peer(settings.video_target_for(peer.id), &peer);
    let mode = state.snapshot().mode;
    if !send_control_to(
        state,
        device_id,
        ControlMessage::SessionOpen { mode, target },
    ) {
        tracing::warn!(%device_id, "发送 SessionOpen 失败");
    }
}

pub fn spawn_accept_loop(state: Arc<AppState>, listener: TcpListener) {
    tauri::async_runtime::spawn(async move {
        loop {
            let (tcp, address) = match listener.accept().await {
                Ok(pair) => pair,
                Err(error) => {
                    tracing::warn!(%error, "接受控制连接失败");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };
            tracing::info!(%address, "接受控制连接");
            let state_clone = Arc::clone(&state);
            tokio::spawn(async move {
                accept_connection(state_clone, tcp, address).await;
            });
        }
    });
}

async fn accept_connection(state: Arc<AppState>, tcp: tokio::net::TcpStream, address: SocketAddr) {
    let config = control_config(&state);
    let event_timeout = config.handshake_timeout;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
    let connection = match nc_control::ControlConnection::accept_stream(tcp, config, event_tx).await
    {
        Ok(connection) => connection,
        Err(error) => {
            tracing::warn!(%error, "入站控制连接握手失败");
            return;
        }
    };
    let mut connection = Some(connection);
    let mut link_id: Option<u64> = None;

    loop {
        let event = match tokio::time::timeout(event_timeout, event_rx.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => {
                remove_incoming_link(&state, link_id);
                return;
            }
            Err(_) => {
                tracing::warn!("入站连接等待事件超时");
                if let Some(connection) = connection.take() {
                    connection.close();
                }
                remove_incoming_link(&state, link_id);
                return;
            }
        };

        if link_id.is_none() {
            let Some(purpose_peer) = incoming_event_peer(&event) else {
                match &event {
                    ControlEvent::Disconnected { reason } => {
                        tracing::debug!(%reason, "入站连接在握手完成前断开");
                    }
                    ControlEvent::PairingFailed { .. } => {
                        let _ = state.events.send(AppEvent::Control {
                            connection_id: u64::MAX,
                            event: event.clone(),
                        });
                    }
                    _ => {}
                }
                if let Some(connection) = connection.take() {
                    connection.close();
                }
                return;
            };
            let Some(connection) = connection.take() else {
                return;
            };
            let id = state.next_generation();
            if let Some(nc_proto::LinkPurpose::Update) = event_purpose(&event) {
                {
                    let mut aux = state.aux_controls.lock();
                    if aux.len() >= 8 {
                        tracing::warn!("更新辅助连接过多，拒绝新连接");
                        connection.close();
                        return;
                    }
                    aux.insert(
                        id,
                        crate::runtime::AuxControl {
                            connection,
                            peer: purpose_peer.clone(),
                        },
                    );
                }
                tracing::info!(device = %purpose_peer.id, "建立更新辅助连接");
                while let Some(event) = event_rx.recv().await {
                    match event {
                        ControlEvent::Message(message) => {
                            handle_update_connection_message(&state, id, message).await;
                        }
                        ControlEvent::Disconnected { reason } => {
                            tracing::debug!(%reason, "更新辅助连接断开");
                            break;
                        }
                        ControlEvent::Heartbeat { rtt_us } => {
                            state.metrics.record_network(rtt_us as f64 / 1000.0, 0.0);
                        }
                        _ => {}
                    }
                }
                state.aux_controls.lock().remove(&id);
                return;
            }

            let local = state.identity.device_id();
            let session_peer = state.session_peer_id();
            let existing = state.links.lock().contains_key(&purpose_peer.id);
            if existing && (session_peer == Some(purpose_peer.id) || local < purpose_peer.id) {
                tracing::debug!(device = %purpose_peer.id, "已有设备组连接，拒绝重复入站连接");
                connection.close();
                return;
            }
            let link = ActiveControl::new(id, connection, ControlDirection::Incoming);
            *link.peer.write() = Some(purpose_peer.clone());
            if let Some(replaced) = state.links.lock().insert(purpose_peer.id, link) {
                replaced.connection.close();
            }
            state.connecting_links.lock().remove(&purpose_peer.id);
            tracing::info!(device = %purpose_peer.id, %address, "建立入站设备组连接");
            link_id = Some(id);
        }

        let Some(id) = link_id else {
            return;
        };
        if state
            .events
            .send(AppEvent::Control {
                connection_id: id,
                event,
            })
            .is_err()
        {
            remove_incoming_link(&state, link_id);
            return;
        }
    }
}

fn remove_incoming_link(state: &AppState, link_id: Option<u64>) {
    let Some(link_id) = link_id else {
        return;
    };
    let mut links = state.links.lock();
    let device = links
        .iter()
        .find(|(_, link)| link.id == link_id)
        .map(|(device, _)| *device);
    if let Some(device) = device {
        links.remove(&device);
    }
}

fn incoming_event_peer(event: &ControlEvent) -> Option<&DeviceInfo> {
    match event {
        ControlEvent::Connected { peer, .. }
        | ControlEvent::PairingSas { peer, .. }
        | ControlEvent::PairingSucceeded { peer } => Some(peer),
        _ => None,
    }
}

fn event_purpose(event: &ControlEvent) -> Option<nc_proto::LinkPurpose> {
    match event {
        ControlEvent::Connected { purpose, .. } => Some(*purpose),
        _ => None,
    }
}

async fn handle_update_connection_message(
    state: &Arc<AppState>,
    connection_id: u64,
    message: ControlMessage,
) {
    match message {
        ControlMessage::UpdateManifest { manifest } => {
            handle_update_manifest(state, manifest, Some(connection_id)).await;
        }
        ControlMessage::UpdateChunk { offset, data, .. } => {
            handle_update_chunk(state, offset, data, Some(connection_id)).await;
        }
        ControlMessage::UpdateComplete { .. } => {
            handle_update_complete(state, Some(connection_id)).await;
        }
        other => {
            tracing::debug!(kind = other.kind_name(), "更新辅助连接忽略非更新消息");
        }
    }
}

fn close_all_links(state: &AppState) {
    let links: Vec<_> = state.links.lock().drain().map(|(_, link)| link).collect();
    for link in links {
        link.connection.close();
    }
    state.connecting_links.lock().clear();
}

async fn cleanup_connection(state: &Arc<AppState>) {
    crate::osd::stop_control_osd(state);
    let previous = state.session_peer_id();
    let was_host = *state.session_role.read() == Some(SessionRole::Host);
    if let Some(previous) = previous {
        send_control_to(state, previous, ControlMessage::SessionClose);
        let file_reason = i18n::file_reason_connection_lost(state.language());
        fail_transfers_for_device(state, previous, &file_reason);
    }
    if was_host {
        broadcast_host_status(state, false);
    } else if let Some(previous) = previous {
        if *state.group_host.read() == Some(previous) {
            set_group_host(state, None);
        }
    }
    *state.session_peer.write() = None;
    *state.session_role.write() = None;
    *state.session_peer_mode.write() = None;
    *state.pending_connect.write() = None;
    set_pending_session(state, None);
    set_switching_target(state, false);
    *state.pending_remote_entry.lock() = None;
    clear_pending_takeover(state);
    state.media.stop_all_outgoing().await;
    state.media.clear_session();
    reset_input_runtime(state);
    state.pairing.lock().take();
    {
        let mut update = state.update.lock();
        if update.incoming.take().is_some() {
            update.status.state = UpdateState::Failed;
            update.status.message = Some(i18n::update_receive_interrupted(state.language()));
            drop(update);
            emit_update_status(state);
        }
    }
    if state.quitting.load(Ordering::SeqCst) {
        close_all_links(state);
    }
    if !state.quitting.load(Ordering::SeqCst) {
        start_discovery(state);
    }
}

fn send_control_to(state: &AppState, device: DeviceId, message: ControlMessage) -> bool {
    let guard = state.links.lock();
    let Some(link) = guard.get(&device) else {
        return false;
    };
    if !link.connected.load(Ordering::SeqCst) {
        return false;
    }
    if let Err(error) = link.connection.send(message) {
        tracing::warn!(%device, %error, "发送控制消息失败");
        return false;
    }
    true
}

fn send_control(state: &AppState, message: ControlMessage) -> bool {
    let Some(device) = state.session_peer_id() else {
        return false;
    };
    send_control_to(state, device, message)
}

fn send_control_on(state: &AppState, connection_id: Option<u64>, message: ControlMessage) -> bool {
    let Some(connection_id) = connection_id else {
        return send_control(state, message);
    };
    if let Some(device) = state.link_device(connection_id) {
        return send_control_to(state, device, message);
    }
    let guard = state.aux_controls.lock();
    let Some(aux) = guard.get(&connection_id) else {
        return false;
    };
    if let Err(error) = aux.connection.send(message) {
        tracing::warn!(%error, "更新连接发送失败");
        return false;
    }
    true
}

async fn handle_control_event(state: &Arc<AppState>, connection_id: u64, event: ControlEvent) {
    if connection_id == u64::MAX {
        if let ControlEvent::PairingFailed { reason } = event {
            dispatch_session(
                state,
                SessionEvent::PairingFailed {
                    reason: reason.clone(),
                },
            )
            .await;
            emit_pairing_failed(state, None, reason);
        }
        return;
    }

    let Some(device) = state.link_device(connection_id) else {
        return;
    };

    match event {
        ControlEvent::Connected {
            peer,
            trusted: _,
            certificate_fingerprint: _,
            media_key,
            purpose,
        } => {
            {
                let mut links = state.links.lock();
                if let Some(control) = links.get_mut(&device) {
                    *control.peer.write() = Some(peer.clone());
                    *control.media_key.write() = Some(media_key);
                    control.connected.store(true, Ordering::SeqCst);
                }
            }
            state.connecting_links.lock().remove(&device);
            if *state.pending_connect.read() == Some(device) {
                *state.pending_connect.write() = None;
            }
            state.reconnect_attempt.store(0, Ordering::SeqCst);
            save_peer_device(state, &peer);
            emit_devices(state);
            tracing::info!(device = %peer.id, ?purpose, "设备组连接可用");
            send_control_to(
                state,
                device,
                ControlMessage::TopologySync {
                    nodes: local_display_nodes(state),
                },
            );
            let shared = state
                .settings
                .read()
                .shared_snapshot(state.identity.device_id());
            send_shared_config_to(state, device, &shared);
            emit_shared_sync_status(state);
            if *state.group_host.read() == Some(state.identity.device_id()) {
                send_control_to(
                    state,
                    device,
                    ControlMessage::HostStatus {
                        host: state.identity.device_id(),
                        active: true,
                    },
                );
            }
            if *state.pending_session.read() == Some(device) {
                send_session_open(state, device);
            }
        }
        ControlEvent::PairingSas { peer, sas } => {
            let outgoing = *state.pending_connect.read() == Some(peer.id);
            let busy = state.snapshot().state.is_active()
                || state.session_peer_id().is_some()
                || state.input.lock().controlling_remote;
            if !outgoing && busy {
                if let Some(link) = state.links.lock().get(&peer.id) {
                    let _ = link.connection.confirm_sas(false);
                }
                let language = state.language();
                let (title, body) = i18n::pairing_rejected_busy(language, &peer.name);
                add_priority_notification(
                    state,
                    Severity::Warning,
                    NotificationCategory::Pairing,
                    title,
                    body,
                    None,
                );
                return;
            }
            dispatch_session(state, SessionEvent::PairingStarted { device: peer.id }).await;
            {
                let mut pairing = state.pairing.lock();
                let entry = pairing.get_or_insert_with(|| PairingState {
                    device_id: Some(peer.id),
                    device_name: Some(peer.name.clone()),
                    sas: None,
                });
                entry.device_id = Some(peer.id);
                entry.device_name = Some(peer.name.clone());
                entry.sas = Some(sas.clone());
            }
            dispatch_session(state, SessionEvent::SasRequired).await;
            ipc::emit(
                &state.app,
                ipc::EVENT_PAIRING,
                PairingEventPayload {
                    stage: PairingStage::SasRequired,
                    device_id: Some(peer.id),
                    device_name: Some(peer.name.clone()),
                    sas: Some(sas.clone()),
                    message: None,
                },
            );
            if !outgoing {
                let language = state.language();
                let (title, body) = i18n::pairing_requested(language, &peer.name);
                add_priority_notification(
                    state,
                    Severity::Warning,
                    NotificationCategory::Pairing,
                    title,
                    body,
                    None,
                );
            }
            open_control_layer(state, false).await;
        }
        ControlEvent::PairingSucceeded { peer } => {
            dispatch_session(state, SessionEvent::PairingSucceeded { device: peer.id }).await;
            state.pairing.lock().take();
            ipc::emit(
                &state.app,
                ipc::EVENT_PAIRING,
                PairingEventPayload {
                    stage: PairingStage::Success,
                    device_id: Some(peer.id),
                    device_name: Some(peer.name.clone()),
                    sas: None,
                    message: None,
                },
            );
            emit_devices(state);
            emit_shared_sync_status(state);
        }
        ControlEvent::PairingFailed { reason } => {
            remove_link(state, device);
            *state.pending_connect.write() = None;
            set_pending_session(state, None);
            dispatch_session(
                state,
                SessionEvent::PairingFailed {
                    reason: reason.clone(),
                },
            )
            .await;
            emit_pairing_failed(state, None, reason);
        }
        ControlEvent::Message(message) => {
            handle_control_message(state, connection_id, message).await;
        }
        ControlEvent::Heartbeat { rtt_us } => {
            let loss = state.metrics.snapshot().loss_percent;
            state.metrics.record_network(rtt_us as f64 / 1000.0, loss);
        }
        ControlEvent::RemoteClock { offset_us } => {
            state.record_clock_offset(device, offset_us);
        }
        ControlEvent::Disconnected { reason } => {
            let was_session = state.session_peer_id() == Some(device);
            let peer = state.link_peer(device);
            let was_pairing = state
                .pairing
                .lock()
                .as_ref()
                .and_then(|pairing| pairing.device_id)
                == Some(device);
            let was_host = was_session && *state.session_role.read() == Some(SessionRole::Host);
            let file_reason = i18n::file_reason_connection_lost(state.language());
            remove_link(state, device);
            fail_transfers_for_device(state, device, &file_reason);
            if *state.group_host.read() == Some(device) {
                set_group_host(state, None);
            }
            if was_host {
                broadcast_host_status(state, false);
            }
            state.shared_sync.lock().acks.remove(&device);
            emit_shared_sync_status(state);
            emit_devices(state);
            if was_session {
                *state.session_peer.write() = None;
                *state.session_role.write() = None;
                *state.session_peer_mode.write() = None;
                state.media.clear_session();
                state.media.stop_all_outgoing().await;
                set_pending_session(state, Some(device));
                dispatch_session(
                    state,
                    SessionEvent::ConnectionLost {
                        reason: reason.clone(),
                    },
                )
                .await;
                schedule_reconnect(state, peer).await;
            } else if was_pairing {
                set_pending_session(state, None);
                dispatch_session(
                    state,
                    SessionEvent::PairingFailed {
                        reason: reason.clone(),
                    },
                )
                .await;
                emit_pairing_failed(state, Some(device), reason);
            } else {
                tracing::debug!(%device, %reason, "设备组连接断开");
                schedule_group_reconnect(state, device);
            }
        }
    }
}

fn schedule_group_reconnect(state: &Arc<AppState>, device: DeviceId) {
    if state.user_disconnect.load(Ordering::SeqCst) {
        return;
    }
    let state_clone = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        ensure_group_link(&state_clone, device, PairingIntent::Trusted, false).await;
    });
}

fn remove_link(state: &AppState, device: DeviceId) {
    if let Some(link) = state.links.lock().remove(&device) {
        link.connection.close();
    }
}

fn emit_pairing_failed(state: &AppState, device_id: Option<DeviceId>, reason: String) {
    ipc::emit(
        &state.app,
        ipc::EVENT_PAIRING,
        PairingEventPayload {
            stage: PairingStage::Failed,
            device_id,
            device_name: None,
            sas: None,
            message: Some(reason),
        },
    );
}

fn save_peer_device(state: &AppState, peer: &DeviceInfo) {
    let discovered = { state.devices.read().get(&peer.id).cloned() };
    if let Some(discovered) = discovered {
        let updated = DiscoveredDevice {
            info: peer.clone(),
            address: discovered.address,
            last_seen_ms: discovered.last_seen_ms,
        };
        state.devices.write().insert(peer.id, updated);
    }
    emit_devices(state);
}

fn peer_media_address(state: &AppState, peer: &DeviceInfo) -> Option<SocketAddr> {
    let ip = state.devices.read().get(&peer.id)?.address.ip();
    (peer.media_port != 0).then(|| SocketAddr::new(ip, peer.media_port))
}

fn current_remote_device(state: &AppState) -> Option<DeviceId> {
    let snapshot = state.snapshot();
    snapshot
        .current_display_device
        .or_else(|| snapshot.peer.as_ref().map(|peer| peer.id))
}

async fn schedule_reconnect(state: &Arc<AppState>, peer: Option<DeviceInfo>) {
    if state.user_disconnect.load(Ordering::SeqCst) {
        return;
    }
    let Some(peer) = peer else {
        return;
    };
    let attempt = state.reconnect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
    if attempt > RECONNECT_MAX_ATTEMPTS {
        state.reconnect_attempt.store(0, Ordering::SeqCst);
        *state.pending_connect.write() = None;
        set_pending_session(state, None);
        set_switching_target(state, false);
        let _ = state
            .events
            .send(AppEvent::Session(SessionEvent::ReconnectTick {
                attempt: (RECONNECT_MAX_ATTEMPTS + 1) as u32,
                max: RECONNECT_MAX_ATTEMPTS as u32,
            }));
        return;
    }
    *state.pending_connect.write() = Some(peer.id);
    let state_clone = Arc::clone(state);
    let generation = state.generation();
    let delay_ms = (500_u64.saturating_mul(attempt)).min(3_000);
    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        let _ = state_clone.events.send(AppEvent::ReconnectWake {
            generation,
            attempt: attempt as u32,
            max: RECONNECT_MAX_ATTEMPTS as u32,
        });
    });
    if let Some(previous) = state.reconnect_task.lock().replace(handle) {
        previous.abort();
    }
}

#[cfg(target_os = "macos")]
fn ensure_target_accessibility(state: &AppState) {
    static NOTIFIED: AtomicBool = AtomicBool::new(false);
    if state
        .permissions
        .read()
        .get(nc_core::PermissionKind::Accessibility)
        == Some(true)
    {
        return;
    }
    if NOTIFIED.swap(true, Ordering::SeqCst) {
        return;
    }
    tracing::warn!("缺少辅助功能授权：远端键鼠注入与本机输入捕获不可用");
    let granted = nc_platform::request_permission(nc_core::PermissionKind::Accessibility)
        .unwrap_or(false);
    if granted {
        return;
    }
    let language = state.language();
    let (title, body) = i18n::target_accessibility_missing(language);
    add_notification(
        state,
        Severity::Warning,
        NotificationCategory::Permission,
        title,
        body,
        Some(NotificationAction {
            kind: NotificationActionKind::OpenSettings,
            label: i18n::open_settings_action(language),
            payload: None,
        }),
    );
}

async fn handle_control_message(
    state: &Arc<AppState>,
    connection_id: u64,
    message: ControlMessage,
) {
    let Some(device) = state.link_device(connection_id) else {
        return;
    };
    let is_session = state.session_peer_id() == Some(device);
    match &message {
        ControlMessage::SessionOpen { mode, target } => {
            handle_session_open(state, connection_id, device, *mode, *target).await;
            return;
        }
        ControlMessage::SessionAccept { session_id } => {
            handle_session_accept(state, device, session_id.0).await;
            return;
        }
        ControlMessage::SessionReject { reason } => {
            handle_session_reject(state, device, reason.clone()).await;
            return;
        }
        ControlMessage::SessionClose => {
            if is_session {
                close_remote_session(state).await;
            }
            return;
        }
        _ => {}
    }
    if !is_session {
        match &message {
            ControlMessage::TopologySync { .. }
            | ControlMessage::HostStatus { .. }
            | ControlMessage::HostReleaseRequest { .. }
            | ControlMessage::SharedConfig { .. }
            | ControlMessage::SharedConfigAck { .. }
            | ControlMessage::PermissionSync { .. }
            | ControlMessage::Notify { .. } => {}
            ControlMessage::UpdateManifest { .. }
            | ControlMessage::UpdateChunk { .. }
            | ControlMessage::UpdateComplete { .. } => {
                handle_update_connection_message(state, connection_id, message).await;
                return;
            }
            ControlMessage::FileOffer { .. }
            | ControlMessage::FileAccept { .. }
            | ControlMessage::FileReject { .. }
            | ControlMessage::FileChunk { .. }
            | ControlMessage::FileComplete { .. }
            | ControlMessage::FileResult { .. } => {}
            _ => return,
        }
    }
    match message {
        ControlMessage::ModeChange { mode } => {
            *state.session_peer_mode.write() = Some(mode);
            if mode == Mode::Aggregation {
                crate::osd::start_control_osd(state);
            } else {
                crate::osd::stop_control_osd(state);
            }
        }
        ControlMessage::Input { envelope } => {
            let secure_desktop = state.secure_desktop.load(Ordering::SeqCst);
            let service_injection = *state.session_role.read() == Some(SessionRole::Target)
                && input_service_available();
            if secure_desktop && !service_injection {
                return;
            }
            #[cfg(target_os = "macos")]
            if *state.session_role.read() == Some(SessionRole::Target) {
                ensure_target_accessibility(state);
            }
            if let Some(peer) = current_remote_device(state) {
                let owner_was_local = state.snapshot().input_owner.is_local();
                if owner_was_local {
                    state.input.lock().remote_handover_notified = true;
                    dispatch_session(
                        state,
                        SessionEvent::PeerInputOwner(nc_core::InputOwner::Remote {
                            device_id: peer,
                        }),
                    )
                    .await;
                } else if state.input.lock().note_remote_handover() {
                    handle_notice(state, SessionNotice::ControlHandover);
                }
            }
            crate::osd::start_control_osd(state);
            let inject_result = {
                let mut input = state.input.lock();
                input.last_remote_event_ms = crate::notifications::now_ms();
                input.remote_input_released = false;
                input.inject(&envelope.event, secure_desktop)
            };
            match inject_result {
                Ok(()) => {
                    if *state.session_role.read() == Some(SessionRole::Target) {
                        if let InputEvent::MouseMove { x, y, .. } = &envelope.event {
                            track_remote_display_node(state, *x, *y).await;
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "注入远端输入失败，释放输入所有权");
                    reset_input_runtime(state);
                    send_control(state, ControlMessage::ReleaseAll);
                    dispatch_session(state, SessionEvent::ReleaseAll).await;
                    let language = state.language();
                    let (title, body) = i18n::input_injection_blocked(language);
                    add_notification(
                        state,
                        Severity::Warning,
                        NotificationCategory::System,
                        title,
                        body,
                        None,
                    );
                }
            }
        }
        ControlMessage::ReleaseAll => {
            tracing::info!("收到远端释放输入");
            crate::osd::stop_control_osd(state);
            reset_input_runtime(state);
            state.input.lock().remote_input_released = true;
            if *state.group_host.read() == Some(device) {
                set_group_host(state, None);
            }
            dispatch_session(state, SessionEvent::ReturnToLocal).await;
        }
        ControlMessage::EdgeReturn { direction } => {
            if !state.snapshot().input_owner.is_local() {
                {
                    let mut input = state.input.lock();
                    input.set_relative_mode(false);
                    input.set_suppressed(false);
                    input.end_remote_cursor();
                    input.control_started_ms = 0;
                    input.release_all();
                    input.remote_handover_notified = false;
                }
                dispatch_session(state, SessionEvent::ReturnToLocal).await;
                warp_cursor_to_edge(state, direction);
            }
        }
        ControlMessage::Clipboard { content } => {
            if state.settings.read().clipboard_enabled {
                apply_remote_clipboard(state, content);
            }
        }
        ControlMessage::FileOffer {
            transfer_id,
            file_name,
            size,
            sha256,
        } => {
            if !state.settings.read().file_transfer_enabled {
                send_control_to(
                    state,
                    device,
                    ControlMessage::FileReject {
                        transfer_id,
                        reason: i18n::file_reason_disabled(state.language()),
                    },
                );
                return;
            }
            let destination = state.paths.downloads_dir.clone();
            match FileReceiver::new(
                transfer_id,
                &file_name,
                size,
                sha256,
                destination,
                FILE_CHUNK_SIZE,
            ) {
                Ok(receiver) => {
                    let id = state.transfers.lock().next_id();
                    let view = crate::ipc::TransferView {
                        id,
                        direction: crate::ipc::TransferDirection::Receive,
                        file_name: file_name.clone(),
                        bytes_done: 0,
                        bytes_total: size,
                        state: crate::ipc::TransferState::Transferring,
                        message: None,
                    };
                    state.transfers.lock().entries.insert(
                        id,
                        TransferEntry::Receiving {
                            device_id: device,
                            transfer_id,
                            receiver: Some(receiver),
                            view,
                        },
                    );
                    send_control_to(
                        state,
                        device,
                        ControlMessage::FileAccept {
                            transfer_id,
                            offset: 0,
                        },
                    );
                    emit_transfers(state);
                }
                Err(error) => {
                    send_control_to(
                        state,
                        device,
                        ControlMessage::FileReject {
                            transfer_id,
                            reason: error.to_string(),
                        },
                    );
                }
            }
        }
        ControlMessage::FileAccept {
            transfer_id,
            offset,
        } => {
            if let Some(id) = find_sending_transfer(state, device, transfer_id) {
                let mut start = false;
                if let Some(TransferEntry::Sending { sender, view, .. }) =
                    state.transfers.lock().entries.get_mut(&id)
                {
                    if offset == 0 {
                        view.state = crate::ipc::TransferState::Transferring;
                        view.bytes_done = sender.progress().bytes_done;
                        start = true;
                    } else {
                        view.state = crate::ipc::TransferState::Failed;
                        view.message = Some(i18n::file_reason_resume_unsupported(state.language()));
                    }
                }
                emit_transfers(state);
                if start {
                    let _ = state.events.send(AppEvent::TransferTick { id });
                }
            }
        }
        ControlMessage::FileReject {
            transfer_id,
            reason,
        } => {
            if let Some(entry) = find_sending_transfer(state, device, transfer_id) {
                if let Some(entry) = state.transfers.lock().entries.get_mut(&entry) {
                    entry.view_mut().state = crate::ipc::TransferState::Failed;
                    entry.view_mut().message = Some(reason.clone());
                }
                add_notification(
                    state,
                    Severity::Error,
                    NotificationCategory::FileTransfer,
                    i18n::file_failed(state.language(), &reason).0,
                    i18n::file_failed(state.language(), &reason).1,
                    None,
                );
                emit_transfers(state);
            }
        }
        ControlMessage::FileChunk {
            transfer_id,
            offset,
            data,
        } => {
            handle_file_chunk(state, device, transfer_id, offset, data).await;
        }
        ControlMessage::FileComplete { transfer_id } => {
            handle_file_complete(state, device, transfer_id).await;
        }
        ControlMessage::FileResult {
            transfer_id,
            success,
            message,
        } => {
            if let Some(id) = find_sending_transfer(state, device, transfer_id) {
                let cancelled = !success && message == "cancelled";
                let file_name = {
                    let mut transfers = state.transfers.lock();
                    let Some(entry) = transfers.entries.get_mut(&id) else {
                        return;
                    };
                    entry.view_mut().state = if success {
                        crate::ipc::TransferState::Done
                    } else if cancelled {
                        crate::ipc::TransferState::Cancelled
                    } else {
                        crate::ipc::TransferState::Failed
                    };
                    entry.view_mut().message = Some(message.clone());
                    let total = entry.view().bytes_total;
                    if success {
                        entry.view_mut().bytes_done = total;
                    }
                    entry.view().file_name.clone()
                };
                emit_transfers(state);
                if success {
                    let language = state.language();
                    let (title, body) = i18n::file_sent(language, &file_name);
                    add_notification(
                        state,
                        Severity::Success,
                        NotificationCategory::FileTransfer,
                        title,
                        body,
                        None,
                    );
                } else if !cancelled {
                    let language = state.language();
                    let (title, body) = i18n::file_failed(language, &message);
                    add_notification(
                        state,
                        Severity::Error,
                        NotificationCategory::FileTransfer,
                        title,
                        body,
                        None,
                    );
                }
            } else if let Some(id) = find_receiving_transfer(state, device, transfer_id) {
                let mut transfers = state.transfers.lock();
                if let Some(TransferEntry::Receiving { receiver, view, .. }) =
                    transfers.entries.get_mut(&id)
                {
                    if let Some(receiver) = receiver.as_mut() {
                        receiver.abort();
                    }
                    view.state = if message == "cancelled" {
                        crate::ipc::TransferState::Cancelled
                    } else {
                        crate::ipc::TransferState::Failed
                    };
                    view.message = Some(message.clone());
                }
                drop(transfers);
                emit_transfers(state);
            }
        }
        ControlMessage::UpdateManifest { manifest } => {
            handle_update_manifest(state, manifest, Some(connection_id)).await;
        }
        ControlMessage::UpdateChunk { offset, data, .. } => {
            handle_update_chunk(state, offset, data, Some(connection_id)).await;
        }
        ControlMessage::UpdateComplete { .. } => {
            handle_update_complete(state, Some(connection_id)).await;
        }
        ControlMessage::UpdateDecision { .. } | ControlMessage::UpdateResult { .. } => {}
        ControlMessage::TopologySync { nodes } => {
            merge_remote_topology(state, nodes, device);
        }
        ControlMessage::HostStatus { host, active } => {
            apply_remote_host_status(state, device, host, active);
        }
        ControlMessage::HostReleaseRequest { requester } => {
            handle_host_release_request(state, device, requester).await;
        }
        ControlMessage::SharedConfig { snapshot } => {
            handle_shared_config(state, snapshot, device).await;
        }
        ControlMessage::SharedConfigAck { revision } => {
            state.shared_sync.lock().acks.insert(device, revision);
            emit_shared_sync_status(state);
        }
        ControlMessage::StreamRequest { request } => {
            let state_clone = Arc::clone(state);
            tokio::spawn(async move {
                handle_stream_request(&state_clone, request).await;
            });
        }
        ControlMessage::StreamStop { stream_id } => {
            state.media.stop_outgoing(stream_id).await;
        }
        ControlMessage::KeyframeRequest { stream_id } => {
            state.media.request_keyframe(stream_id);
        }
        ControlMessage::StreamInfo {
            stream_id,
            device_id,
            width,
            height,
            fps,
            monitor_index,
        } => {
            state
                .media
                .bind_video(stream_id, device_id, width, height, fps);
            if width > 0 && height > 0 {
                let mut input = state.input.lock();
                if input.absolute_pointer && input.remote_device == Some(device_id) {
                    input.stream_aspect = Some(width as f64 / height as f64);
                }
            }
            ipc::emit(
                &state.app,
                "stream-info",
                serde_json::json!({
                    "stream_id": stream_id.0,
                    "device_id": device_id.to_hex(),
                    "width": width,
                    "height": height,
                    "fps": fps,
                    "monitor_index": monitor_index,
                }),
            );
        }
        ControlMessage::AudioStreamInfo {
            stream_id,
            device_id,
            codec,
            sample_rate,
            channels,
        } => {
            state.media.bind_audio(
                stream_id,
                device_id,
                AudioBinding {
                    codec,
                    sample_rate,
                    channels,
                },
            );
        }
        ControlMessage::PermissionSync { report } => {
            *state.permissions.write() = report.clone();
            ipc::emit(&state.app, ipc::EVENT_PERMISSION_CHANGED, report);
        }
        ControlMessage::Notify { notification } => {
            if let Some(marker) = notification
                .action
                .as_ref()
                .and_then(|action| action.payload.as_deref())
            {
                if marker == "secure_desktop_active" || marker == "secure_desktop_inactive" {
                    let snapshot = state.snapshot();
                    if snapshot.mode == Mode::Aggregation && snapshot.stage_visible {
                        if let Some(peer) = state.peer() {
                            tracing::info!("安全桌面状态变化，重启远端媒体流");
                            start_media(state, peer.id).await;
                        }
                    }
                    return;
                }
            }
            let added = add_notification(
                state,
                notification.severity,
                notification.category,
                notification.title,
                notification.body,
                notification.action,
            );
            let _ = added;
        }
        ControlMessage::SessionOpen { .. }
        | ControlMessage::SessionAccept { .. }
        | ControlMessage::SessionReject { .. }
        | ControlMessage::SessionClose
        | ControlMessage::Heartbeat { .. }
        | ControlMessage::HeartbeatAck { .. }
        | ControlMessage::Ack { .. }
        | ControlMessage::Goodbye { .. }
        | ControlMessage::Error { .. } => {}
    }
}

async fn handle_session_open(
    state: &Arc<AppState>,
    connection_id: u64,
    device: DeviceId,
    mode: Mode,
    _target: VideoTarget,
) {
    let Some(peer) = state.link_peer(device) else {
        return;
    };
    let role = *state.session_role.read();
    let released = state.input.lock().remote_input_released;
    clear_released_host_lock(state, role, released);
    if group_host_blocks(*state.group_host.read(), state.identity.device_id())
        && *state.group_host.read() != Some(device)
    {
        tracing::warn!(%device, "拒绝会话打开：设备组内已有活动主机");
        let _ = send_control_to(
            state,
            device,
            ControlMessage::SessionReject {
                reason: "设备组内已有活动主机".into(),
            },
        );
        return;
    }
    if let Some(current) = state.session_peer_id() {
        if current != device {
            if local_edge_takeover(role, released) {
                let _ = send_control_to(state, current, ControlMessage::SessionClose);
                close_remote_session(state).await;
            } else {
                tracing::warn!(%device, %current, "拒绝会话打开：本机已有活动控制会话");
                let _ = send_control_to(
                    state,
                    device,
                    ControlMessage::SessionReject {
                        reason: "本机已有活动控制会话".into(),
                    },
                );
                return;
            }
        }
    }
    if stale_host_role(
        *state.session_role.read(),
        state.session_peer_id().is_some(),
    ) {
        tracing::info!(%device, "清除没有活动会话的主机角色");
        *state.session_role.write() = None;
    }
    let pending = *state.pending_session.read();
    if *state.session_role.read() == Some(SessionRole::Host)
        || (pending.is_some() && pending != Some(device))
    {
        tracing::warn!(%device, ?pending, "拒绝会话打开：本机正在发起其他控制");
        let _ = send_control_to(
            state,
            device,
            ControlMessage::SessionReject {
                reason: "本机正在发起其他控制".into(),
            },
        );
        return;
    }
    let snapshot = state.snapshot();
    if snapshot.state.is_active() && snapshot.peer.as_ref().map(|peer| peer.id) != Some(device) {
        tracing::warn!(%device, ?snapshot.peer, "拒绝会话打开：本机正在控制其他设备");
        let _ = send_control_to(
            state,
            device,
            ControlMessage::SessionReject {
                reason: "本机正在控制其他设备".into(),
            },
        );
        return;
    }

    *state.session_peer.write() = Some(device);
    *state.session_role.write() = Some(SessionRole::Target);
    *state.session_peer_mode.write() = Some(mode);
    state.input.lock().remote_input_released = false;
    let session_id = state.next_generation();
    start_session_media(state, connection_id, device, &peer, session_id);
    let accepted = send_control_to(
        state,
        device,
        ControlMessage::SessionAccept {
            session_id: nc_core::SessionId(session_id),
        },
    );
    if !accepted {
        *state.session_peer.write() = None;
        *state.session_role.write() = None;
        *state.session_peer_mode.write() = None;
        state.media.clear_session();
        return;
    }
    set_group_host(state, Some(device));
    if snapshot.state.is_active() && snapshot.peer.as_ref().map(|peer| peer.id) == Some(device) {
        return;
    }
    state.input.lock().remote_handover_notified = false;
    dispatch_session(
        state,
        SessionEvent::ConnectionEstablished { peer: peer.clone() },
    )
    .await;
    crate::osd::start_control_osd(state);
    if !state.permissions.read().all_granted() {
        dispatch_session(state, SessionEvent::PermissionLost).await;
    }
}

async fn handle_session_accept(state: &Arc<AppState>, device: DeviceId, session_id: u64) {
    if *state.pending_session.read() != Some(device) {
        return;
    }
    tracing::info!(%device, session_id, "收到会话接受");
    let Some(peer) = state.link_peer(device) else {
        return;
    };
    let Some(connection_id) = state.links.lock().get(&device).map(|link| link.id) else {
        return;
    };
    let previous_peer = state.snapshot().peer.as_ref().map(|peer| peer.id);
    let was_active = state.snapshot().state.is_active();
    *state.session_peer.write() = Some(device);
    *state.session_role.write() = Some(SessionRole::Host);
    set_pending_session(state, None);
    clear_pending_takeover(state);
    broadcast_host_status(state, true);
    start_session_media(state, connection_id, device, &peer, session_id);
    state.input.lock().remote_handover_notified = false;
    if was_active && previous_peer != Some(device) {
        dispatch_session(state, SessionEvent::SwitchTarget { peer: peer.clone() }).await;
    } else {
        dispatch_session(
            state,
            SessionEvent::ConnectionEstablished { peer: peer.clone() },
        )
        .await;
    }
    let was_switching = state.switching_target.load(Ordering::SeqCst);
    let initialized = complete_pending_remote_entry(state, device).await;
    if was_switching {
        if initialized {
            set_switching_target(state, false);
        } else {
            reset_input_runtime(state);
            state.input.lock().set_suppressed(true);
            let state_clone = Arc::clone(state);
            tauri::async_runtime::spawn(async move {
                ensure_remote_stage_input(&state_clone).await;
                set_switching_target(&state_clone, false);
            });
        }
    }
    if !state.permissions.read().all_granted() {
        dispatch_session(state, SessionEvent::PermissionLost).await;
    }
}

async fn complete_pending_remote_entry(state: &Arc<AppState>, device: DeviceId) -> bool {
    let pending = state.pending_remote_entry.lock().take();
    let Some(pending) = pending.filter(|pending| pending.device == device) else {
        return false;
    };
    let target = {
        let layout = state.layout.read();
        layout.node(&pending.node).cloned()
    };
    let Some(target) = target else {
        return false;
    };
    let initial = begin_remote_node_input(
        state,
        &target,
        pending.entry,
        pending.scale,
        pending.direction,
        pending.cursor_offset,
        true,
    );
    dispatch_session(
        state,
        SessionEvent::EdgeCrossed {
            to_device: device,
            to_node: pending.node,
            direction: pending.direction,
        },
    )
    .await;
    if !forward_input(state, initial, now_us()) {
        abort_remote_takeover(state).await;
    }
    true
}

async fn handle_session_reject(state: &Arc<AppState>, device: DeviceId, reason: String) {
    if *state.pending_session.read() != Some(device) {
        return;
    }
    tracing::warn!(%device, %reason, "目标拒绝会话");
    let was_host = *state.session_role.read() == Some(SessionRole::Host);
    set_pending_session(state, None);
    set_switching_target(state, false);
    *state.pending_remote_entry.lock() = None;
    *state.session_role.write() = None;
    *state.session_peer_mode.write() = None;
    if was_host {
        broadcast_host_status(state, false);
    }
    let was_active = state.snapshot().state.is_active();
    if was_active {
        *state.session_peer.write() = None;
        *state.session_peer_mode.write() = None;
        state.media.clear_session();
        state.media.stop_all_outgoing().await;
        dispatch_session(state, SessionEvent::RemoteSessionClosed).await;
    } else {
        dispatch_session(
            state,
            SessionEvent::ConnectionLost {
                reason: reason.clone(),
            },
        )
        .await;
    }
    let language = state.language();
    let (title, body) = i18n::session_rejected(language, &reason);
    add_notification(
        state,
        Severity::Warning,
        NotificationCategory::Connection,
        title,
        body,
        None,
    );
}

async fn handle_host_release_request(state: &Arc<AppState>, sender: DeviceId, requester: DeviceId) {
    if sender != requester {
        tracing::warn!(%sender, %requester, "忽略来源不一致的主机释放请求");
        return;
    }
    let trusted = state
        .trust
        .read()
        .map(|store| store.get(&requester).is_some())
        .unwrap_or(false);
    if !trusted {
        tracing::warn!(%requester, "忽略未受信任设备的主机释放请求");
        return;
    }
    let local = state.identity.device_id();
    let is_host = *state.group_host.read() == Some(local);
    tracing::info!(%requester, is_host, "收到主机释放请求");
    if !is_host {
        let _ = send_control_to(
            state,
            requester,
            ControlMessage::HostStatus {
                host: local,
                active: false,
            },
        );
        return;
    }
    let peer = state.session_peer_id();
    if let Some(peer) = peer {
        if state.input.lock().controlling_remote {
            send_control_to(state, peer, ControlMessage::ReleaseAll);
        }
        send_control_to(state, peer, ControlMessage::SessionClose);
    }
    crate::osd::stop_control_osd(state);
    *state.session_peer.write() = None;
    *state.session_role.write() = None;
    *state.session_peer_mode.write() = None;
    *state.pending_connect.write() = None;
    set_pending_session(state, None);
    *state.pending_remote_entry.lock() = None;
    clear_pending_takeover(state);
    set_switching_target(state, false);
    reset_input_runtime(state);
    state.media.clear_session();
    state.media.stop_all_outgoing().await;
    broadcast_host_status(state, false);
    dispatch_session(state, SessionEvent::RemoteSessionClosed).await;
    let language = state.language();
    let name = device_display_name(state, requester);
    let (title, body) = i18n::host_takeover_released(language, &name);
    add_notification(
        state,
        Severity::Info,
        NotificationCategory::Connection,
        title,
        body,
        None,
    );
}

async fn close_remote_session(state: &Arc<AppState>) {
    crate::osd::stop_control_osd(state);
    let previous = state.session_peer_id();
    let was_host = *state.session_role.read() == Some(SessionRole::Host);
    if let Some(previous) = previous {
        if *state.group_host.read() == Some(previous) {
            set_group_host(state, None);
        }
    }
    if was_host {
        broadcast_host_status(state, false);
    }
    *state.session_peer.write() = None;
    *state.session_role.write() = None;
    *state.session_peer_mode.write() = None;
    *state.pending_remote_entry.lock() = None;
    state.media.clear_session();
    state.media.stop_all_outgoing().await;
    reset_input_runtime(state);
    dispatch_session(state, SessionEvent::RemoteSessionClosed).await;
}

fn start_session_media(
    state: &AppState,
    connection_id: u64,
    device: DeviceId,
    peer: &DeviceInfo,
    session_id: u64,
) {
    let media_key = state
        .links
        .lock()
        .get(&device)
        .and_then(|link| link.media_key());
    let Some(media_key) = media_key else {
        tracing::warn!(%device, "会话缺少媒体密钥");
        return;
    };
    state.media.start_receiver(
        connection_id,
        media_key,
        session_id,
        state.events.clone(),
    );
    if let Some(peer_addr) = peer_media_address(state, peer) {
        state.media.start_sender(peer_addr, media_key, session_id);
    }
}

fn apply_remote_clipboard(state: &AppState, content: ClipboardContent) {
    let payload = match content {
        ClipboardContent::Text(text) => ClipboardPayload::Text(text),
        ClipboardContent::ImagePng(bytes) => ClipboardPayload::ImagePng(bytes),
    };
    let hash = nc_clipboard::hash_payload(&payload);
    let mut guard = state.clipboard.lock();
    let Some(watcher) = guard.as_mut() else {
        return;
    };
    match watcher.set_remote(payload) {
        Ok(()) => {
            *state.clipboard_last_hash.write() = Some(hash);
            drop(guard);
        }
        Err(error) => tracing::warn!(%error, "写入远端剪贴板失败"),
    }
}

async fn poll_clipboard(state: &Arc<AppState>) {
    if !state.settings.read().clipboard_enabled || !state.snapshot().state.is_active() {
        return;
    }
    let payload = {
        let mut guard = state.clipboard.lock();
        let Some(watcher) = guard.as_mut() else {
            return;
        };
        match watcher.poll() {
            Ok(payload) => payload,
            Err(error) => {
                let _ = state
                    .events
                    .send(AppEvent::ClipboardError(error.to_string()));
                return;
            }
        }
    };
    let Some(payload) = payload else {
        return;
    };
    let hash = nc_clipboard::hash_payload(&payload);
    if state.clipboard_last_hash.read().as_ref() == Some(&hash) {
        return;
    }
    *state.clipboard_last_hash.write() = Some(hash);
    if let Err(error) = state.events.send(AppEvent::Clipboard(payload)) {
        tracing::debug!(%error, "投递剪贴板事件失败");
    }
}

async fn handle_media_event(state: &Arc<AppState>, connection_id: u64, event: MediaEvent) {
    let session_connection = state
        .session_peer_id()
        .and_then(|device| state.links.lock().get(&device).map(|link| link.id));
    if session_connection != Some(connection_id) {
        return;
    }
    if let MediaEvent::Stats(stats) = &event {
        state.metrics.record_loss(stats.loss_percent as f64);
        if stats.loss_percent > 0.1 && state.media.fec_parity() == 0 {
            state.media.set_fec_parity(1);
            tracing::info!(loss = stats.loss_percent, "媒体丢包超过阈值，启用 FEC");
        }
    }
    if matches!(&event, MediaEvent::Nack { .. }) {
        state.metrics.record_nack();
    }
    if let MediaEvent::Frame(frame) = &event {
        if !frame.is_audio {
            let mut wait = state.first_frame_wait.lock();
            if let Some((stream_id, started)) = *wait {
                if stream_id == frame.stream_id {
                    state
                        .metrics
                        .record_first_frame(started.elapsed().as_secs_f64() * 1000.0);
                    *wait = None;
                }
            }
        }
    }
    state.media.handle_event(event, &state.bridge).await;
}

fn broadcast_topology(state: &AppState) {
    let nodes = local_display_nodes(state);
    let devices: Vec<DeviceId> = state.links.lock().keys().copied().collect();
    for device in devices {
        send_control_to(
            state,
            device,
            ControlMessage::TopologySync {
                nodes: nodes.clone(),
            },
        );
    }
}

fn current_monitor_index(state: &AppState) -> Option<u32> {
    let layout = state.layout.read();
    state
        .snapshot()
        .current_display_node
        .as_ref()
        .and_then(|node| layout.node(node))
        .filter(|node| !node.is_local)
        .or_else(|| {
            let peer = state.snapshot().peer?;
            layout
                .nodes
                .iter()
                .filter(|node| {
                    node.device_id == peer.id && !node.is_local && layout.is_active(&node.id)
                })
                .min_by_key(|node| (!node.is_primary, node.monitor_index))
        })
        .map(|node| node.monitor_index)
}

async fn start_media(state: &Arc<AppState>, target: DeviceId) {
    let local = state.identity.device_id();
    if target == local {
        stop_media(state).await;
        return;
    }
    {
        let mut adaptive = state.adaptive.lock();
        adaptive.level = 0;
        adaptive.bad_ticks = 0;
        adaptive.good_ticks = 0;
    }
    let Some(peer) = state.peer() else {
        return;
    };
    if peer.id != target {
        return;
    }
    let settings = state.settings.read().clone();
    let target_video = target_for_peer(settings.video_target_for(peer.id), &peer);
    if state.media.sender().is_none() {
        let key = state
            .links
            .lock()
            .get(&target)
            .and_then(|link| link.media_key());
        if let Some(key) = key {
            if let Some(addr) = peer_media_address(state, &peer) {
                let session_id = state.media.session_id();
                state.media.start_sender(addr, key, session_id);
            }
        }
    }
    if state.media.sender().is_none() {
        return;
    }
    *state.first_frame_wait.lock() = Some((StreamId::VIDEO, Instant::now()));
    send_control(
        state,
        ControlMessage::StreamRequest {
            request: StreamRequest {
                stream_id: StreamId::VIDEO,
                target: target_video,
                audio: settings.audio_enabled,
                monitor_index: current_monitor_index(state),
            },
        },
    );
    state.media.request_all_keyframes();
    state.bridge.send_control(&serde_json::json!({
        "type": "stream_started",
        "device_id": target.to_hex(),
        "stream_id": StreamId::VIDEO.0,
    }));
}

fn target_for_peer(target: VideoTarget, peer: &DeviceInfo) -> VideoTarget {
    let capabilities = &peer.capabilities;
    let max_width = if capabilities.max_width >= 2 {
        capabilities.max_width
    } else {
        3840
    };
    let max_height = if capabilities.max_height >= 2 {
        capabilities.max_height
    } else {
        2160
    };
    let max_fps = if capabilities.max_fps > 0 {
        capabilities.max_fps
    } else {
        60
    };
    target
        .with_recommended_bitrate()
        .clamped_to(max_width, max_height, max_fps)
}

fn fit_target_to_monitor(target: VideoTarget, monitor: &nc_platform::MonitorInfo) -> VideoTarget {
    let (capture_width, capture_height) = monitor.capture_size();
    let monitor_width = capture_width.max(2);
    let monitor_height = capture_height.max(2);
    let monitor_portrait = monitor_height > monitor_width;
    let target_portrait = target.height > target.width;
    let (target_width, target_height) = if monitor_portrait != target_portrait {
        (target.height, target.width)
    } else {
        (target.width, target.height)
    };
    let scale = (target_width as f64 / monitor_width as f64)
        .min(target_height as f64 / monitor_height as f64)
        .min(1.0);
    let fitted_width = ((monitor_width as f64 * scale).round() as u32).clamp(2, monitor_width) & !1;
    let fitted_height =
        ((monitor_height as f64 * scale).round() as u32).clamp(2, monitor_height) & !1;
    VideoTarget {
        width: fitted_width,
        height: fitted_height,
        ..target
    }
}

fn target_for_level(base: VideoTarget, level: u8) -> VideoTarget {
    let factor = match level {
        0 => 1.0,
        1 => 0.85,
        2 => 0.7,
        3 => 0.55,
        _ => 0.4,
    };
    VideoTarget {
        bitrate_kbps: ((base.bitrate_kbps as f64 * factor) as u32).max(1_000),
        ..base
    }
}

fn maybe_adapt(state: &Arc<AppState>) {
    let snapshot = state.snapshot();
    if snapshot.state != nc_core::SessionState::Aggregating || !snapshot.stage_visible {
        return;
    }
    if state.media.sender().is_none() {
        return;
    }
    let metrics = state.metrics.snapshot();
    if metrics.frames_sent == 0 && metrics.frames_received == 0 {
        return;
    }
    let base = match state.peer() {
        Some(peer) => target_for_peer(state.settings.read().video_target_for(peer.id), &peer),
        None => state.settings.read().target.with_recommended_bitrate(),
    };
    let mut adaptive = state.adaptive.lock();
    let under_pressure = metrics.loss_percent > 2.0;
    let stable = metrics.loss_percent < 0.5;

    if under_pressure {
        adaptive.bad_ticks = adaptive.bad_ticks.saturating_add(1);
        adaptive.good_ticks = 0;
    } else if stable {
        adaptive.good_ticks = adaptive.good_ticks.saturating_add(1);
        adaptive.bad_ticks = 0;
    } else {
        adaptive.bad_ticks = 0;
        adaptive.good_ticks = 0;
    }

    let mut next_level = adaptive.level;
    if adaptive.bad_ticks >= 3 && adaptive.level < 4 {
        next_level = adaptive.level + 1;
    } else if adaptive.good_ticks >= 20 && adaptive.level > 0 {
        next_level = adaptive.level - 1;
    }
    if next_level == adaptive.level {
        return;
    }
    adaptive.level = next_level;
    adaptive.bad_ticks = 0;
    adaptive.good_ticks = 0;
    let next = target_for_level(base, next_level);
    drop(adaptive);
    *state.first_frame_wait.lock() = Some((StreamId::VIDEO, Instant::now()));
    send_control(
        state,
        ControlMessage::StreamRequest {
            request: StreamRequest {
                stream_id: StreamId::VIDEO,
                target: next,
                audio: state.settings.read().audio_enabled,
                monitor_index: current_monitor_index(state),
            },
        },
    );
    tracing::debug!(
        level = next_level,
        bitrate_kbps = next.bitrate_kbps,
        "自适应码率调整"
    );
}

async fn stop_media(state: &Arc<AppState>) {
    send_control(
        state,
        ControlMessage::StreamStop {
            stream_id: StreamId::VIDEO,
        },
    );
    state.media.stop_all_outgoing().await;
    state.media.bindings.write().clear();
    *state.last_stream_request.lock() = None;
    *state.first_frame_wait.lock() = None;
    state.bridge.send_control(&serde_json::json!({
        "type": "stream_stopped",
        "stream_id": StreamId::VIDEO.0,
    }));
}

async fn handle_stream_request(state: &Arc<AppState>, request: StreamRequest) {
    if request.stream_id == StreamId::AUDIO {
        return;
    }
    *state.last_stream_request.lock() = Some(request);
    let Some(sender) = state.media.sender() else {
        return;
    };
    crate::osd::refresh_control_osd_window_id(&state.app);
    let monitors = nc_platform::monitors().unwrap_or_default();
    let monitor_index = request
        .monitor_index
        .and_then(|index| {
            monitors
                .get(index as usize)
                .map(|monitor| (index, monitor.clone()))
        })
        .or_else(|| {
            monitors
                .iter()
                .position(|monitor| monitor.primary)
                .map(|index| (index as u32, monitors[index].clone()))
        })
        .or_else(|| {
            nc_platform::primary_monitor()
                .ok()
                .flatten()
                .map(|monitor| (0, monitor))
        });
    let Some((monitor_index, monitor)) = monitor_index else {
        return;
    };
    let device_id = state.identity.device_id();
    let limited = state.video_capabilities.limit_target(request.target);
    let target = fit_target_to_monitor(limited, &monitor);
    let with_audio = request.audio;
    let descriptor = match state
        .media
        .start_outgoing(
            OutgoingRequest {
                stream_id: request.stream_id,
                device_id,
                monitor_id: monitor.id,
                target,
                with_audio,
                sender,
                metrics: Arc::clone(&state.metrics),
                excluded_window_id: crate::osd::control_osd_window_id(),
                encoder_vendor: state.video_capabilities.preferred_encoder_vendor,
                prefer_speed: state.video_capabilities.class
                    == crate::runtime::EncoderClass::Software,
            },
            state.events.clone(),
        )
        .await
    {
        Ok(descriptor) => descriptor,
        Err(error) => {
            tracing::warn!(%error, "启动发送流失败");
            let (title, body) = i18n::media_start_failed(state.language(), &error.to_string());
            send_remote_notice(
                state,
                Severity::Error,
                NotificationCategory::Media,
                &title,
                &body,
            );
            add_notification(
                state,
                Severity::Error,
                NotificationCategory::Media,
                title,
                body,
                None,
            );
            return;
        }
    };
    send_control(
        state,
        ControlMessage::StreamInfo {
            stream_id: request.stream_id,
            device_id,
            width: target.width,
            height: target.height,
            fps: target.fps,
            monitor_index: Some(monitor_index),
        },
    );
    if let Some(descriptor) = descriptor {
        send_control(
            state,
            ControlMessage::AudioStreamInfo {
                stream_id: StreamId::AUDIO,
                device_id,
                codec: descriptor.codec,
                sample_rate: descriptor.sample_rate,
                channels: descriptor.channels,
            },
        );
    }
}

fn warp_cursor_to_edge(state: &AppState, direction: Direction) {
    let use_work_area = state.settings.read().edge_inset;
    let layout = state.layout.read().clone();
    let monitors = state.monitors.read().clone();
    let local = state.identity.device_id();
    let cursor = nc_platform::cursor_position()
        .ok()
        .map(|(x, y)| Point::new(x as f64, y as f64));
    let node_id = cursor
        .and_then(|cursor| {
            local_cursor_in_layout(&layout, local, &monitors, cursor, use_work_area)
                .map(|(id, _)| id)
        })
        .or_else(|| {
            layout
                .nodes
                .iter()
                .find(|node| {
                    node.device_id == local
                        && node.is_local
                        && layout.is_active(&node.id)
                        && node.is_primary
                })
                .or_else(|| {
                    layout.nodes.iter().find(|node| {
                        node.device_id == local && node.is_local && layout.is_active(&node.id)
                    })
                })
                .map(|node| node.id.clone())
        });
    let Some(node_id) = node_id else {
        return;
    };
    let Some(node) = layout.node(&node_id) else {
        return;
    };
    let bounds = node.bounds();
    let point = match direction {
        Direction::Left => Point::new(bounds.right() - 2.0, bounds.center().y),
        Direction::Right => Point::new(bounds.x + 2.0, bounds.center().y),
        Direction::Up => Point::new(bounds.center().x, bounds.bottom() - 2.0),
        Direction::Down => Point::new(bounds.center().x, bounds.y + 2.0),
    };
    if let Some((x, y)) =
        layout_point_to_screen(&layout, local, &monitors, &node_id, point, use_work_area)
    {
        let _ = nc_platform::set_cursor_position(x, y);
    } else {
        let _ = nc_platform::set_cursor_position(point.x as i32, point.y as i32);
    }
}

fn captured_shortcut_action(state: &AppState, event: &InputEvent) -> Option<(UiAction, u32)> {
    let InputEvent::Key {
        state: key_state,
        key_code,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    if *key_state != KeyState::Down {
        return None;
    }
    const MODIFIER_MASK: u8 = 0b1111;
    let settings = state.settings.read();
    crate::shortcuts::local_shortcuts(&settings.shortcuts)
        .into_iter()
        .find(|shortcut| {
            shortcut.key_code == *key_code
                && (modifiers.0 & MODIFIER_MASK) == shortcut.modifiers.0
        })
        .map(|shortcut| (shortcut.action, shortcut.key_code))
}

async fn handle_input(state: &Arc<AppState>, captured: nc_input::CapturedInput) {
    if state.secure_desktop.load(Ordering::SeqCst) {
        return;
    }
    if !captured.injected {
        if let InputEvent::Key {
            state: KeyState::Up,
            key_code,
            ..
        } = &captured.event
        {
            let pending = {
                let mut input = state.input.lock();
                if input.shortcut_key_down == Some(*key_code) {
                    input.shortcut_key_down = None;
                    input.pending_shortcut_action.take()
                } else {
                    None
                }
            };
            if let Some(action) = pending {
                tracing::info!("本机快捷键触发（松开）");
                send_control(state, ControlMessage::ReleaseAll);
                handle_action(state, action).await;
                return;
            }
        }
    }
    let snapshot = state.snapshot();
    let role = *state.session_role.read();
    let injected = captured.injected || state.input.lock().is_recent_injection(&captured.event);
    if !injected && role == Some(SessionRole::Target) {
        crate::osd::note_control_osd_local_input(state);
    }
    let released = state.input.lock().remote_input_released;
    clear_released_host_lock(state, role, released);
    let local_flow = local_flow_allowed(state, &snapshot, role, released);
    if !local_flow && !snapshot.state.is_active() {
        return;
    }
    if role == Some(SessionRole::Target) && !local_flow {
        return;
    }
    if state.input.lock().controlling_remote && nc_input::escape_release_pressed(&captured) {
        emergency_release(state).await;
        return;
    }
    if injected || captured.local_only {
        return;
    }
    if state.switching_target.load(Ordering::SeqCst) {
        if nc_input::is_escape_key_down(&captured.event) && register_double_escape(state) {
            tracing::info!("切换目标期间双击 Esc，取消切换");
            abort_pending_session(state, "用户取消切换", false).await;
            dispatch_session(state, SessionEvent::StageHide).await;
        }
        return;
    }
    if state.input.lock().remote_paused {
        return;
    }

    if let nc_core::InputOwner::Remote { .. } = snapshot.input_owner {
        if !state.input.lock().controlling_remote {
            return;
        }
        state.input.lock().last_remote_event_ms = crate::notifications::now_ms();
        if let InputEvent::Key {
            state: key_state, ..
        } = &captured.event
        {
            if nc_input::is_escape_key_down(&captured.event) {
                if register_double_escape(state) {
                    let direction = state.input.lock().remote_entry_direction;
                    release_remote_input(state, direction, None, None).await;
                    return;
                }
            } else if nc_input::is_escape_key_up(&captured.event) {
                state.input.lock().escape_down = false;
            } else if *key_state == KeyState::Down {
                state.input.lock().last_escape_ms = 0;
            }
        }
        if state.input.lock().is_suppressed() {
            if let Some((action, key_code)) = captured_shortcut_action(state, &captured.event) {
                let mut input = state.input.lock();
                if input.shortcut_key_down == Some(key_code) {
                    return;
                }
                input.shortcut_key_down = Some(key_code);
                input.pending_shortcut_action = Some(action);
                return;
            }
        }
        if state.input.lock().absolute_pointer {
            match &captured.event {
                InputEvent::MouseMove { .. }
                | InputEvent::MouseButton { .. }
                | InputEvent::MouseWheel { .. } => {
                    if let Some(forwarded) = map_absolute_stage_event(state, &captured.event) {
                        if !forward_input(state, forwarded, captured.timestamp_us) {
                            abort_remote_takeover(state).await;
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        if captured.raw_relative {
            if matches!(&captured.event, InputEvent::MouseMove { .. })
                && handle_remote_relative_move(state, &captured.event, captured.timestamp_us).await
            {
                return;
            }
            return;
        }
        if matches!(&captured.event, InputEvent::MouseMove { .. }) {
            return;
        }
        if let Some(forwarded) = map_pointer_to_virtual_cursor(state, &captured.event) {
            if !forward_input(state, forwarded, captured.timestamp_us) {
                abort_remote_takeover(state).await;
            }
        }
        return;
    }

    if captured.raw_relative {
        return;
    }

    if state.pending_session.read().is_none() {
        if let Some(crossing) = detect_edge_crossing(state, &captured.event) {
            handle_edge_crossing(state, crossing).await;
            return;
        }
    }

    if snapshot.mode == Mode::Aggregation && snapshot.stage_visible {
        if let InputEvent::MouseMove { x, y, .. } = &captured.event {
            if let Some((node_id, direction)) = edge_direction_at_cursor(state, *x, *y) {
                if local_has_no_peer_on(state, &node_id, direction) {
                    dispatch_session(state, SessionEvent::StageHide).await;
                }
            }
        }
    }
}

async fn handle_remote_relative_move(
    state: &Arc<AppState>,
    event: &InputEvent,
    timestamp_us: u64,
) -> bool {
    let InputEvent::MouseMove {
        delta_x,
        delta_y,
        modifiers,
        ..
    } = event
    else {
        return false;
    };
    let mut edge_candidate = None;
    let mut forwarded = None;
    let mut should_emit_cursor = false;
    {
        let mut input = state.input.lock();
        let (Some((mut x, mut y)), Some((width, height))) =
            (input.virtual_cursor, input.virtual_bounds)
        else {
            return false;
        };
        let (scale_x, scale_y) = input.remote_scale;
        let offset = input.remote_cursor_offset;
        let scaled_dx = *delta_x as f64 * scale_x;
        let scaled_dy = *delta_y as f64 * scale_y;
        x = (x + scaled_dx).clamp(0.0, (width - 1.0).max(0.0));
        y = (y + scaled_dy).clamp(0.0, (height - 1.0).max(0.0));
        input.virtual_cursor = Some((x, y));

        let now = crate::notifications::now_ms();
        if !input.remote_inside && remote_inside_any(x, y, width, height) {
            input.remote_inside = true;
        }
        let can_leave = input.remote_inside || now.saturating_sub(input.control_started_ms) >= 300;
        let direction = remote_edge_direction(x, y, width, height, *delta_x, *delta_y);
        if can_leave {
            if let Some(direction) = direction {
                if input.remote_outward_direction == Some(direction) {
                    input.remote_outward_events = input.remote_outward_events.saturating_add(1);
                } else {
                    input.remote_outward_direction = Some(direction);
                    input.remote_outward_events = 1;
                }
                if input.remote_outward_events >= 2 {
                    input.remote_outward_events = 0;
                    input.remote_outward_direction = None;
                    edge_candidate = Some((direction, x, y, input.remote_node.clone()));
                }
            } else {
                input.remote_outward_events = 0;
                input.remote_outward_direction = None;
            }
        } else {
            input.remote_outward_events = 0;
            input.remote_outward_direction = None;
        }

        if edge_candidate.is_none() {
            should_emit_cursor =
                now.saturating_sub(input.last_cursor_emit_ms) >= CURSOR_EMIT_INTERVAL_MS;
            if should_emit_cursor {
                input.last_cursor_emit_ms = now;
            }
            forwarded = Some(InputEvent::MouseMove {
                x: (x + offset.0).round() as i32,
                y: (y + offset.1).round() as i32,
                delta_x: scaled_dx.round() as i32,
                delta_y: scaled_dy.round() as i32,
                modifiers: *modifiers,
            });
        }
    }

    if let Some((direction, x, y, node)) = edge_candidate {
        let Some(node) = node else {
            return true;
        };
        follow_remote_edge(state, &node, direction, (x, y), timestamp_us, true).await;
        return true;
    }

    if should_emit_cursor {
        emit_remote_cursor(state);
    }
    if let Some(forwarded) = forwarded {
        if !forward_input(state, forwarded, timestamp_us) {
            abort_remote_takeover(state).await;
        }
    }
    true
}

async fn follow_remote_edge(
    state: &Arc<AppState>,
    node: &DisplayNodeId,
    direction: Direction,
    cursor: (f64, f64),
    timestamp_us: u64,
    warp_local: bool,
) -> bool {
    let layout = state.layout.read().clone();
    let Some(source) = layout.node(node).cloned() else {
        return false;
    };
    let Some(target) = layout.neighbor(node, direction).cloned() else {
        return false;
    };
    let (entry, scale, offset) =
        remote_neighbor_entry(&layout, &source, &target, cursor, direction);
    if target.is_local {
        release_remote_input(
            state,
            warp_local.then(|| local_entry_edge(direction)),
            Some(target.id.clone()),
            warp_local.then_some(direction),
        )
        .await;
    } else if state.session_peer_id() == Some(target.device_id) {
        show_edge_handoff_osd(state, direction, &target);
        state.input.lock().release_all();
        let initial =
            begin_remote_node_input(state, &target, entry, scale, direction, offset, true);
        dispatch_session(
            state,
            SessionEvent::EdgeCrossed {
                to_device: target.device_id,
                to_node: target.id.clone(),
                direction,
            },
        )
        .await;
        if !forward_input(state, initial, timestamp_us) {
            abort_remote_takeover(state).await;
        }
    } else {
        show_edge_handoff_osd(state, direction, &target);
        *state.pending_remote_entry.lock() = Some(PendingRemoteEntry {
            device: target.device_id,
            node: target.id.clone(),
            entry,
            scale,
            cursor_offset: offset,
            direction,
        });
        tracing::info!(
            from = %node,
            to = %target.id,
            direction = ?direction,
            "连续流转：切换控制目标"
        );
        switch_session_target(state, target.device_id).await;
    }
    true
}

fn remote_edge_direction(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    delta_x: i32,
    delta_y: i32,
) -> Option<Direction> {
    if x <= 1.0 && delta_x < 0 {
        Some(Direction::Left)
    } else if x >= width - 2.0 && delta_x > 0 {
        Some(Direction::Right)
    } else if y <= 1.0 && delta_y < 0 {
        Some(Direction::Up)
    } else if y >= height - 2.0 && delta_y > 0 {
        Some(Direction::Down)
    } else {
        None
    }
}

fn remote_inside_any(x: f64, y: f64, width: f64, height: f64) -> bool {
    x > 8.0 && x < width - 8.0 && y > 8.0 && y < height - 8.0
}

fn remote_neighbor_entry(
    layout: &Layout,
    source: &DisplayNode,
    target: &DisplayNode,
    cursor: (f64, f64),
    direction: Direction,
) -> (Point, (f64, f64), (f64, f64)) {
    let source_width = source.resolution.width.max(1) as f64;
    let source_height = source.resolution.height.max(1) as f64;
    let target_width = target.resolution.width.max(1) as f64;
    let target_height = target.resolution.height.max(1) as f64;
    let ratio_x = (cursor.0 / source_width).clamp(0.0, 1.0);
    let ratio_y = (cursor.1 / source_height).clamp(0.0, 1.0);
    let entry = match direction {
        Direction::Left => Point::new(target_width - 2.0, ratio_y * target_height),
        Direction::Right => Point::new(1.0, ratio_y * target_height),
        Direction::Up => Point::new(ratio_x * target_width, target_height - 2.0),
        Direction::Down => Point::new(ratio_x * target_width, 1.0),
    };
    let scale = (target_width / source_width, target_height / source_height);
    let offset = remote_cursor_offset(layout, target.device_id, target);
    (entry, scale, offset)
}

async fn track_local_display_node(state: &Arc<AppState>) {
    let Ok((cursor_x, cursor_y)) = nc_platform::cursor_position() else {
        return;
    };
    let node_id = {
        let layout = state.layout.read();
        let monitors = state.monitors.read();
        local_cursor_in_layout(
            &layout,
            state.identity.device_id(),
            &monitors,
            Point::new(cursor_x as f64, cursor_y as f64),
            false,
        )
        .map(|(node_id, _)| node_id)
    };
    let Some(node_id) = node_id else {
        return;
    };
    if state.snapshot().current_display_node.as_ref() == Some(&node_id) {
        return;
    }
    dispatch_session(state, SessionEvent::LocalDisplayNodeChanged { node: node_id }).await;
}

async fn track_remote_display_node(state: &Arc<AppState>, x: i32, y: i32) {
    let node_id = {
        let layout = state.layout.read();
        let monitors = state.monitors.read();
        local_cursor_in_layout(
            &layout,
            state.identity.device_id(),
            &monitors,
            Point::new(x as f64, y as f64),
            false,
        )
        .map(|(node_id, _)| node_id)
    };
    let Some(node_id) = node_id else {
        return;
    };
    if state.snapshot().current_display_node.as_ref() == Some(&node_id) {
        return;
    }
    dispatch_session(state, SessionEvent::RemoteDisplayNodeChanged { node: node_id }).await;
}

async fn check_edge_crossing_tick(state: &Arc<AppState>) {
    if state.secure_desktop.load(Ordering::SeqCst) {
        state.input.lock().edge_detector.clear_pending();
        crate::osd::hide_halo(state);
        return;
    }
    if state.snapshot().input_owner.is_local() {
        track_local_display_node(state).await;
    }
    if !state.settings.read().edge_enabled {
        state.input.lock().edge_detector.clear_pending();
        crate::osd::hide_halo(state);
        return;
    }
    let snapshot = state.snapshot();
    let role = *state.session_role.read();
    let released = state.input.lock().remote_input_released;
    if !local_flow_allowed(state, &snapshot, role, released) {
        crate::osd::hide_halo(state);
        return;
    }
    let Ok((cursor_x, cursor_y)) = nc_platform::cursor_position() else {
        crate::osd::hide_halo(state);
        return;
    };
    if !cursor_in_allowed_edge_area(state, cursor_x as f64, cursor_y as f64) {
        state.input.lock().edge_detector.clear_pending();
        crate::osd::hide_halo(state);
        return;
    }
    let language = state.language();
    let use_work_area = state.settings.read().edge_inset;
    let (crossing, halo) = {
        let layout = state.layout.read();
        let monitors = state.monitors.read();
        let local = state.identity.device_id();
        let Some((node_id, point)) = local_cursor_in_layout(
            &layout,
            local,
            &monitors,
            Point::new(cursor_x as f64, cursor_y as f64),
            use_work_area,
        ) else {
            state.input.lock().edge_detector.reset();
            drop(monitors);
            drop(layout);
            crate::osd::hide_halo(state);
            return;
        };
        let halo = edge_halo_view(&layout, &node_id, point, language);
        let now = crate::notifications::now_ms();
        let mut runtime = state.input.lock();
        let crossing = runtime.edge_detector.update(&layout, &node_id, point, now);
        (crossing, halo)
    };
    if let Some(crossing) = crossing {
        crate::osd::hide_halo(state);
        handle_edge_crossing(state, crossing).await;
    } else if let Some(halo) = halo {
        crate::osd::show_halo(state, halo, use_work_area);
    } else {
        crate::osd::hide_halo(state);
    }
}

fn hovered_edge_direction(node: &DisplayNode, point: Point, tolerance: f64) -> Option<Direction> {
    let bounds = node.bounds();
    let mut best: Option<(f64, Direction)> = None;
    for direction in [
        Direction::Left,
        Direction::Right,
        Direction::Up,
        Direction::Down,
    ] {
        let distance = match direction {
            Direction::Left => (point.x - bounds.x).abs(),
            Direction::Right => (point.x - bounds.right()).abs(),
            Direction::Up => (point.y - bounds.y).abs(),
            Direction::Down => (point.y - bounds.bottom()).abs(),
        };
        if distance <= tolerance && best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, direction));
        }
    }
    best.map(|(_, direction)| direction)
}

fn edge_halo_view(
    layout: &Layout,
    node_id: &DisplayNodeId,
    point: Point,
    language: nc_core::Language,
) -> Option<ipc::EdgeHaloView> {
    let node = layout.node(node_id)?;
    let direction = hovered_edge_direction(node, point, EDGE_HALO_TOLERANCE_PX)?;
    let direction_key = crate::osd::direction_key(direction).to_string();
    match layout.neighbor(node_id, direction) {
        Some(target) if !target.is_local => Some(ipc::EdgeHaloView {
            direction: direction_key,
            title: target.device_name.clone(),
            display: target.display_name.trim().to_string(),
            hint: i18n::edge_halo_target(language),
            available: true,
        }),
        Some(_) => None,
        None => {
            let (title, hint) = i18n::edge_halo_no_device(language);
            Some(ipc::EdgeHaloView {
                direction: direction_key,
                title,
                display: String::new(),
                hint,
                available: false,
            })
        }
    }
}

fn show_edge_handoff_osd(state: &Arc<AppState>, direction: Direction, target: &DisplayNode) {
    crate::osd::hide_halo(state);
    let language = state.language();
    let (kicker, hint) = i18n::edge_osd_handoff(language, state.snapshot().mode);
    let payload = ipc::EdgeOsdView {
        direction: crate::osd::direction_key(direction).to_string(),
        kicker,
        title: target.device_name.clone(),
        display: target.display_name.trim().to_string(),
        hint,
    };
    crate::osd::show(state, Some(direction), payload);
}

fn show_edge_release_osd(state: &Arc<AppState>, direction: Option<Direction>, emergency: bool) {
    crate::osd::hide_halo(state);
    let language = state.language();
    let (kicker, hint) = i18n::edge_osd_release(language, emergency);
    let snapshot = state.snapshot();
    let layout = state.layout.read();
    let node = snapshot
        .current_display_node
        .as_ref()
        .and_then(|id| layout.node(id))
        .or_else(|| {
            layout.nodes.iter().find(|node| {
                node.device_id == state.identity.device_id()
                    && node.is_local
                    && layout.is_active(&node.id)
                    && node.is_primary
            })
        })
        .cloned();
    let payload = ipc::EdgeOsdView {
        direction: crate::osd::direction_key(direction.unwrap_or(Direction::Left)).to_string(),
        kicker,
        title: state.settings.read().device_name.clone(),
        display: node
            .map(|node| node.display_name.trim().to_string())
            .unwrap_or_default(),
        hint,
    };
    crate::osd::show(state, direction, payload);
}

async fn handle_edge_crossing(state: &Arc<AppState>, crossing: EdgeCrossing) {
    let layout = state.layout.read().clone();
    let Some(target) = layout.node(&crossing.to).cloned() else {
        return;
    };
    if target.is_local || !layout.is_active(&target.id) {
        state.input.lock().edge_detector.reset();
        return;
    }
    show_edge_handoff_osd(state, crossing.direction, &target);
    let source = layout.node(&crossing.from).cloned();
    if source.as_ref().is_some_and(|node| node.is_local) {
        state.input.lock().local_entry_node = Some(crossing.from.clone());
    }
    let entry = mapped_entry_point(&layout, &crossing, &target);
    let scale = source
        .as_ref()
        .map(|source| {
            (
                target.resolution.width.max(1) as f64 / source.resolution.width.max(1) as f64,
                target.resolution.height.max(1) as f64 / source.resolution.height.max(1) as f64,
            )
        })
        .unwrap_or((1.0, 1.0));
    let cursor_offset = remote_cursor_offset(&layout, target.device_id, &target);
    let takeover = local_edge_takeover(
        *state.session_role.read(),
        state.input.lock().remote_input_released,
    );
    if takeover || state.session_peer_id() != Some(target.device_id) {
        *state.pending_remote_entry.lock() = Some(PendingRemoteEntry {
            device: target.device_id,
            node: target.id.clone(),
            entry,
            scale,
            cursor_offset,
            direction: crossing.direction,
        });
        switch_session_target(state, target.device_id).await;
        return;
    }
    let initial = begin_remote_node_input(
        state,
        &target,
        entry,
        scale,
        crossing.direction,
        cursor_offset,
        true,
    );
    tracing::info!(
        from = %crossing.from,
        to = %crossing.to,
        direction = ?crossing.direction,
        "边缘穿越：输入移交给远端"
    );
    dispatch_session(
        state,
        SessionEvent::EdgeCrossed {
            to_device: target.device_id,
            to_node: target.id.clone(),
            direction: crossing.direction,
        },
    )
    .await;
    if !forward_input(state, initial, now_us()) {
        tracing::warn!("边缘穿越后无法发送输入，恢复本机输入");
        abort_remote_takeover(state).await;
    }
}

fn begin_remote_input(
    state: &Arc<AppState>,
    peer: DeviceId,
    target: &DisplayNode,
    source: Option<&DisplayNode>,
    entry: Point,
    direction: Direction,
) -> InputEvent {
    let scale = source
        .map(|source| {
            (
                target.resolution.width.max(1) as f64 / source.resolution.width.max(1) as f64,
                target.resolution.height.max(1) as f64 / source.resolution.height.max(1) as f64,
            )
        })
        .unwrap_or((1.0, 1.0));
    let cursor_offset = {
        let layout = state.layout.read();
        remote_cursor_offset(&layout, peer, target)
    };
    begin_remote_node_input(state, target, entry, scale, direction, cursor_offset, false)
}

fn begin_remote_node_input(
    state: &Arc<AppState>,
    target: &DisplayNode,
    entry: Point,
    scale: (f64, f64),
    direction: Direction,
    cursor_offset: (f64, f64),
    warp_entry: bool,
) -> InputEvent {
    let bounds = (
        target.resolution.width.max(1) as f64,
        target.resolution.height.max(1) as f64,
    );
    {
        let mut runtime = state.input.lock();
        runtime.begin_remote_cursor((entry.x, entry.y), bounds, scale, direction, cursor_offset);
        runtime.remote_device = Some(target.device_id);
        runtime.remote_node = Some(target.id.clone());
        runtime.set_relative_mode(true);
        runtime.set_suppressed(true);
        runtime.edge_detector.clear_pending();
    }
    if let Some(initial) =
        enable_absolute_stage_pointer_for(state, target, warp_entry.then_some((entry.x, entry.y)))
    {
        return initial;
    }
    emit_remote_cursor(state);
    InputEvent::MouseMove {
        x: (entry.x + cursor_offset.0).round() as i32,
        y: (entry.y + cursor_offset.1).round() as i32,
        delta_x: 0,
        delta_y: 0,
        modifiers: nc_core::Modifiers::NONE,
    }
}

fn emit_remote_cursor(state: &AppState) {
    let snapshot = state.snapshot();
    if snapshot.mode != Mode::Aggregation || !snapshot.stage_visible {
        return;
    }
    let position = {
        let input = state.input.lock();
        input.remote_cursor_fraction()
    };
    if let Some((device_id, x, y)) = position {
        ipc::emit(
            &state.app,
            ipc::EVENT_REMOTE_CURSOR,
            ipc::RemoteCursorView { device_id, x, y },
        );
    }
}

async fn toggle_remote_pause(state: &Arc<AppState>) {
    let (controlling, paused) = {
        let input = state.input.lock();
        (input.controlling_remote, input.remote_paused)
    };
    if !controlling {
        return;
    }
    if paused {
        resume_remote_control(state).await;
    } else {
        pause_remote_control(state).await;
    }
}

async fn pause_remote_control(state: &Arc<AppState>) {
    if !pause_remote_control_now(state) {
        return;
    }
    tracing::info!("暂停远端控制：本机键鼠恢复，远端连接与画面保持");
}

fn pause_remote_control_now(state: &Arc<AppState>) -> bool {
    let paused = { state.input.lock().pause_remote_control() };
    if !paused {
        return false;
    }
    forward_input(state, InputEvent::ReleaseAll, now_us());
    emit_remote_pause(state, true);
    true
}

pub fn pause_remote_for_shortcut_capture(state: &Arc<AppState>) {
    if pause_remote_control_now(state) {
        tracing::info!("录制快捷键：已暂停远端控制");
    }
}

async fn resume_remote_control(state: &Arc<AppState>) {
    let resumed = { state.input.lock().resume_remote_control() };
    if !resumed {
        return;
    }
    let snapshot = state.snapshot();
    let absolute_initial = if snapshot.mode == Mode::Aggregation && snapshot.stage_visible {
        enable_absolute_stage_pointer(state)
    } else {
        None
    };
    match absolute_initial {
        Some(initial) => {
            if !forward_input(state, initial, now_us()) {
                abort_remote_takeover(state).await;
                emit_remote_pause(state, false);
                return;
            }
        }
        None => {
            let input = state.input.lock();
            input.set_relative_mode(true);
            input.set_suppressed(true);
        }
    }
    tracing::info!("恢复远端控制");
    emit_remote_pause(state, false);
}

fn emit_remote_pause(state: &AppState, paused: bool) {
    let shortcut = state.settings.read().shortcuts.pause_remote.clone();
    ipc::emit(
        &state.app,
        ipc::EVENT_REMOTE_PAUSE,
        ipc::RemotePauseView { paused, shortcut },
    );
}

fn reset_input_runtime(state: &AppState) {
    let mut input = state.input.lock();
    let was_paused = input.remote_paused;
    input.set_relative_mode(false);
    input.set_suppressed(false);
    input.shortcut_key_down = None;
    input.pending_shortcut_action = None;
    input.end_remote_cursor();
    input.control_started_ms = 0;
    input.edge_detector.clear_pending();
    input.release_all();
    input.remote_handover_notified = false;
    drop(input);
    if was_paused {
        emit_remote_pause(state, false);
    }
}

fn register_double_escape(state: &AppState) -> bool {
    let now = crate::notifications::now_ms();
    let mut input = state.input.lock();
    if input.escape_down {
        return false;
    }
    input.escape_down = true;
    let double = input.last_escape_ms != 0 && now.saturating_sub(input.last_escape_ms) <= 500;
    input.last_escape_ms = if double { 0 } else { now };
    double
}

async fn release_remote_input(
    state: &Arc<AppState>,
    warp_edge: Option<Direction>,
    local_node: Option<DisplayNodeId>,
    exit_direction: Option<Direction>,
) {
    let (
        virtual_cursor,
        virtual_bounds,
        outward_events,
        inside,
        local_entry_node,
        absolute_pointer,
    ) = {
        let input = state.input.lock();
        (
            input.virtual_cursor,
            input.virtual_bounds,
            input.remote_outward_events,
            input.remote_inside,
            input.local_entry_node.clone(),
            input.absolute_pointer,
        )
    };
    tracing::info!(
        ?warp_edge,
        ?exit_direction,
        ?virtual_cursor,
        outward_events,
        inside,
        ?local_entry_node,
        absolute_pointer,
        "释放远端输入"
    );
    let along_fraction =
        warp_edge.and_then(|edge| warp_along_fraction(edge, virtual_cursor, virtual_bounds));
    reset_input_runtime(state);
    send_control(state, ControlMessage::ReleaseAll);
    let warp_preferred = local_node.as_ref().or(local_entry_node.as_ref());
    let warp_node = warp_edge
        .and_then(|edge| warp_local_cursor_inside(state, edge, warp_preferred, along_fraction));
    if absolute_pointer {
        if let (Some(direction), Some(_)) = (exit_direction, warp_node.as_ref()) {
            state
                .input
                .lock()
                .latch_mouse_moves(direction, STAGE_EXIT_MOVE_LATCH_MS);
        }
    }
    dispatch_session(state, SessionEvent::ReturnToLocal).await;
    if let Some(node) = local_node.or(warp_node).or(local_entry_node) {
        dispatch_session(state, SessionEvent::LocalDisplayNodeChanged { node }).await;
    }
    show_edge_release_osd(state, warp_edge, false);
}

fn stage_target_node(state: &AppState, device_id: DeviceId) -> Option<DisplayNode> {
    let layout = state.layout.read();
    layout
        .nodes
        .iter()
        .find(|node| {
            node.device_id == device_id
                && !node.is_local
                && layout.is_active(&node.id)
                && node.is_primary
        })
        .or_else(|| {
            layout.nodes.iter().find(|node| {
                node.device_id == device_id && !node.is_local && layout.is_active(&node.id)
            })
        })
        .cloned()
}

fn stage_monitor_bounds(state: &AppState) -> Option<(f64, f64, f64, f64)> {
    if let Some(bounds) = stage_target_monitor_bounds(state) {
        return Some(bounds);
    }
    let window = crate::window::stage_window(&state.app)
        .or_else(|| crate::window::main_window(&state.app))?;
    let monitor = window.current_monitor().ok().flatten()?;
    let position = monitor.position();
    let size = monitor.size();
    #[cfg(target_os = "macos")]
    let scale = monitor.scale_factor();
    #[cfg(target_os = "macos")]
    let bounds = (
        position.x as f64 / scale,
        position.y as f64 / scale,
        size.width as f64 / scale,
        size.height as f64 / scale,
    );
    #[cfg(not(target_os = "macos"))]
    let bounds = (
        position.x as f64,
        position.y as f64,
        size.width as f64,
        size.height as f64,
    );
    (bounds.2 > 1.0 && bounds.3 > 1.0).then_some(bounds)
}

fn stage_target_monitor_bounds(state: &AppState) -> Option<(f64, f64, f64, f64)> {
    let entry_node = if state.snapshot().input_owner.is_local() {
        None
    } else {
        state.input.lock().local_entry_node.clone()
    };
    let entry_node = entry_node.and_then(|node_id| state.layout.read().node(&node_id).cloned());
    let monitors = state.monitors.read().clone();
    if let Some(node) = entry_node.filter(|node| node.is_local) {
        if let Some(monitor) = monitor_for_local_node(&node, &monitors) {
            let bounds = monitor.bounds;
            return Some((
                bounds.x as f64,
                bounds.y as f64,
                bounds.width as f64,
                bounds.height as f64,
            ));
        }
    }
    let (x, y) = nc_platform::cursor_position().ok()?;
    let monitor = monitors
        .iter()
        .find(|monitor| point_in_rect(monitor.bounds, x as f64, y as f64))?;
    let bounds = monitor.bounds;
    Some((
        bounds.x as f64,
        bounds.y as f64,
        bounds.width as f64,
        bounds.height as f64,
    ))
}

fn stage_stream_aspect(state: &AppState, device_id: DeviceId) -> f64 {
    if let Some(binding) = state.media.binding(StreamId::VIDEO) {
        if binding.device_id == device_id && binding.width > 0 && binding.height > 0 {
            return binding.width as f64 / binding.height as f64;
        }
    }
    let target = state.settings.read().video_target_for(device_id);
    if target.width > 0 && target.height > 0 {
        target.width as f64 / target.height as f64
    } else {
        16.0 / 9.0
    }
}

fn current_stage_target_node(state: &AppState, device_id: DeviceId) -> Option<DisplayNode> {
    let recorded = state.input.lock().remote_node.clone();
    if let Some(node) = recorded.and_then(|id| state.layout.read().node(&id).cloned()) {
        if node.device_id == device_id {
            return Some(node);
        }
    }
    stage_target_node(state, device_id)
}

fn enable_absolute_stage_pointer(state: &Arc<AppState>) -> Option<InputEvent> {
    let peer = state.peer()?;
    let target = current_stage_target_node(state, peer.id)?;
    let preferred = {
        let input = state.input.lock();
        input
            .virtual_cursor
            .filter(|_| input.remote_node.as_ref() == Some(&target.id))
    };
    enable_absolute_stage_pointer_for(state, &target, preferred)
}

fn enable_absolute_stage_pointer_for(
    state: &Arc<AppState>,
    target: &DisplayNode,
    preferred_entry: Option<(f64, f64)>,
) -> Option<InputEvent> {
    let snapshot = state.snapshot();
    if snapshot.mode != Mode::Aggregation || !snapshot.stage_visible {
        return None;
    }
    let device_id = target.device_id;
    if state.peer().is_none_or(|peer| peer.id != device_id) {
        return None;
    }
    if !state.input.lock().controlling_remote {
        return None;
    }
    let stage_bounds = stage_monitor_bounds(state)?;
    let stream_aspect = stage_stream_aspect(state, device_id);
    let target_bounds = (
        target.resolution.width.max(1) as f64,
        target.resolution.height.max(1) as f64,
    );
    let entry = match preferred_entry {
        Some(entry) => {
            if let Some((x, y)) = crate::runtime::remote_point_to_stage(
                stage_bounds,
                stream_aspect,
                target_bounds,
                entry,
            ) {
                let _ = nc_platform::set_cursor_position(x.round() as i32, y.round() as i32);
            }
            entry
        }
        None => {
            let (cursor_x, cursor_y) = nc_platform::cursor_position().ok()?;
            crate::runtime::stage_pointer_to_remote(
                stage_bounds,
                stream_aspect,
                target_bounds,
                cursor_x as f64,
                cursor_y as f64,
            )?
        }
    };
    let layout = state.layout.read().clone();
    let offset = remote_cursor_offset(&layout, device_id, target);
    let geometry = StagePointerGeometry {
        stage_bounds,
        stream_aspect,
        remote_bounds: target_bounds,
        cursor_offset: offset,
    };
    {
        let mut input = state.input.lock();
        if !input.controlling_remote {
            return None;
        }
        input.begin_absolute_stage_cursor(geometry, entry, device_id, target.id.clone());
    }
    tracing::info!(
        device = %device_id,
        node = %target.id,
        stage_x = stage_bounds.0,
        stage_y = stage_bounds.1,
        stage_width = stage_bounds.2,
        stage_height = stage_bounds.3,
        stream_aspect,
        entry_x = entry.0,
        entry_y = entry.1,
        "全屏视图：绝对指针"
    );
    emit_remote_cursor(state);
    Some(InputEvent::MouseMove {
        x: (entry.0 + offset.0).round() as i32,
        y: (entry.1 + offset.1).round() as i32,
        delta_x: 0,
        delta_y: 0,
        modifiers: nc_core::Modifiers::NONE,
    })
}

async fn ensure_remote_stage_input(state: &Arc<AppState>) {
    if state.secure_desktop.load(Ordering::SeqCst) {
        return;
    }
    for _ in 0..10 {
        let snapshot = state.snapshot();
        if snapshot.state != nc_core::SessionState::Aggregating || !snapshot.stage_visible {
            return;
        }
        let nc_core::InputOwner::Remote { device_id } = snapshot.input_owner else {
            return;
        };
        let Some(peer) = state.peer() else {
            return;
        };
        if peer.id != device_id {
            return;
        }
        if state.input.lock().controlling_remote {
            if let Some(initial) = enable_absolute_stage_pointer(state) {
                if !forward_input(state, initial, now_us()) {
                    tracing::warn!("全屏视图无法切换绝对指针，恢复本机输入");
                    reset_input_runtime(state);
                    let _ = state
                        .events
                        .send(AppEvent::Session(SessionEvent::ReturnToLocal));
                }
            }
            return;
        }
        if let Some(initial) = try_begin_stage_input(state, peer.id, device_id) {
            tracing::info!(device = %device_id, "全屏视图：输入移交给远端");
            if !forward_input(state, initial, now_us()) {
                tracing::warn!("全屏视图无法发送输入，恢复本机输入");
                reset_input_runtime(state);
                let _ = state
                    .events
                    .send(AppEvent::Session(SessionEvent::ReturnToLocal));
            }
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    tracing::warn!("全屏视图无法定位远端显示节点，释放输入并回到本机");
    reset_input_runtime(state);
    let _ = state
        .events
        .send(AppEvent::Session(SessionEvent::ReturnToLocal));
}

fn try_begin_stage_input(
    state: &Arc<AppState>,
    peer: DeviceId,
    device_id: DeviceId,
) -> Option<InputEvent> {
    let layout = state.layout.read().clone();
    let target = layout
        .nodes
        .iter()
        .find(|node| {
            node.device_id == device_id
                && !node.is_local
                && layout.is_active(&node.id)
                && node.is_primary
        })
        .or_else(|| {
            layout.nodes.iter().find(|node| {
                node.device_id == device_id && !node.is_local && layout.is_active(&node.id)
            })
        })
        .cloned()?;
    let local = state.identity.device_id();
    let monitors = state.monitors.read().clone();
    let use_work_area = state.settings.read().edge_inset;
    let screen_cursor = nc_platform::cursor_position()
        .map(|(x, y)| Point::new(x as f64, y as f64))
        .unwrap_or_else(|_| target.center());
    let mapped_local =
        local_cursor_in_layout(&layout, local, &monitors, screen_cursor, use_work_area);
    let source = mapped_local
        .as_ref()
        .and_then(|(node_id, _)| layout.node(node_id))
        .or_else(|| {
            layout
                .nodes
                .iter()
                .find(|node| node.is_local && layout.is_active(&node.id) && node.is_primary)
        })
        .or_else(|| {
            layout
                .nodes
                .iter()
                .find(|node| node.is_local && layout.is_active(&node.id))
        })
        .cloned()?;
    if source.is_local {
        state.input.lock().local_entry_node = Some(source.id.clone());
    }
    let source_point = mapped_local
        .map(|(_, point)| point)
        .unwrap_or(screen_cursor);
    let mapped = layout.map_point(&source.id, &target.id, source_point)?;
    let entry = Point::new(mapped.x - target.origin.x, mapped.y - target.origin.y);
    let direction = relative_direction(&source, &target);
    Some(begin_remote_input(
        state,
        peer,
        &target,
        Some(&source),
        entry,
        direction,
    ))
}

fn relative_direction(from: &DisplayNode, to: &DisplayNode) -> Direction {
    let delta_x = to.center().x - from.center().x;
    let delta_y = to.center().y - from.center().y;
    if delta_x.abs() >= delta_y.abs() {
        if delta_x >= 0.0 {
            Direction::Right
        } else {
            Direction::Left
        }
    } else if delta_y >= 0.0 {
        Direction::Down
    } else {
        Direction::Up
    }
}

async fn abort_remote_takeover(state: &Arc<AppState>) {
    reset_input_runtime(state);
    dispatch_session(state, SessionEvent::ReturnToLocal).await;
}

fn detect_edge_crossing(state: &AppState, event: &InputEvent) -> Option<EdgeCrossing> {
    if !state.settings.read().edge_enabled {
        return None;
    }
    let (x, y) = match event {
        InputEvent::MouseMove { x, y, .. }
        | InputEvent::MouseButton { x, y, .. }
        | InputEvent::MouseWheel { x, y, .. } => (*x as f64, *y as f64),
        _ => return None,
    };
    if !cursor_in_allowed_edge_area(state, x, y) {
        state.input.lock().edge_detector.clear_pending();
        return None;
    }
    let use_work_area = state.settings.read().edge_inset;
    let layout = state.layout.read();
    let monitors = state.monitors.read();
    let local = state.identity.device_id();
    let (node_id, point) =
        local_cursor_in_layout(&layout, local, &monitors, Point::new(x, y), use_work_area)?;
    let now = crate::notifications::now_ms();
    let mut runtime = state.input.lock();
    runtime.edge_detector.update(&layout, &node_id, point, now)
}

fn point_in_rect(rect: nc_platform::RectI, x: f64, y: f64) -> bool {
    x >= rect.x as f64
        && x <= rect.right() as f64
        && y >= rect.y as f64
        && y <= rect.bottom() as f64
}

fn point_in_work_area(monitors: &[nc_platform::MonitorInfo], x: f64, y: f64) -> bool {
    let Some(monitor) = monitors
        .iter()
        .find(|monitor| point_in_rect(monitor.bounds, x, y))
    else {
        return true;
    };
    point_in_rect(monitor.work_area, x, y)
}

fn cursor_in_allowed_edge_area(state: &AppState, x: f64, y: f64) -> bool {
    if !state.settings.read().edge_inset {
        return true;
    }
    point_in_work_area(&state.monitors.read(), x, y)
}

fn local_cursor_in_layout(
    layout: &Layout,
    local: DeviceId,
    monitors: &[nc_platform::MonitorInfo],
    screen: Point,
    use_work_area: bool,
) -> Option<(DisplayNodeId, Point)> {
    if let Some((index, monitor)) = monitors.iter().enumerate().find(|(_, monitor)| {
        let bounds = monitor.bounds;
        screen.x >= bounds.x as f64
            && screen.x <= bounds.right() as f64
            && screen.y >= bounds.y as f64
            && screen.y <= bounds.bottom() as f64
    }) {
        let node = local_node_for_monitor(layout, local, monitor, index)?;
        let usable = if use_work_area {
            monitor.work_area
        } else {
            monitor.bounds
        };
        let fraction_x =
            ((screen.x - usable.x as f64) / usable.width.max(1) as f64).clamp(0.0, 1.0);
        let fraction_y =
            ((screen.y - usable.y as f64) / usable.height.max(1) as f64).clamp(0.0, 1.0);
        return Some((
            node.id.clone(),
            Point::new(
                node.origin.x + fraction_x * node.resolution.width as f64,
                node.origin.y + fraction_y * node.resolution.height as f64,
            ),
        ));
    }
    let node = layout.nodes.iter().find(|node| {
        node.device_id == local
            && node.is_local
            && layout.is_active(&node.id)
            && node.contains(screen)
    })?;
    Some((node.id.clone(), screen))
}

fn local_node_for_monitor<'a>(
    layout: &'a Layout,
    local: DeviceId,
    monitor: &nc_platform::MonitorInfo,
    index: usize,
) -> Option<&'a DisplayNode> {
    let local_nodes: Vec<&DisplayNode> = layout
        .nodes
        .iter()
        .filter(|node| node.device_id == local && node.is_local && layout.is_active(&node.id))
        .collect();
    let monitor_name = monitor.name.trim();
    let by_name: Vec<&DisplayNode> = local_nodes
        .iter()
        .copied()
        .filter(|node| {
            !node.display_name.trim().is_empty() && node.display_name.trim() == monitor_name
        })
        .collect();
    if by_name.len() == 1 {
        return by_name.first().copied();
    }
    local_nodes
        .iter()
        .copied()
        .find(|node| node.monitor_index as usize == index)
        .or_else(|| {
            if monitor.primary {
                local_nodes.iter().copied().find(|node| node.is_primary)
            } else {
                None
            }
        })
        .or_else(|| local_nodes.first().copied())
}

fn monitor_for_local_node<'a>(
    node: &DisplayNode,
    monitors: &'a [nc_platform::MonitorInfo],
) -> Option<&'a nc_platform::MonitorInfo> {
    let node_name = node.display_name.trim();
    if !node_name.is_empty() {
        let by_name: Vec<&nc_platform::MonitorInfo> = monitors
            .iter()
            .filter(|monitor| monitor.name.trim() == node_name)
            .collect();
        if by_name.len() == 1 {
            return by_name.first().copied();
        }
    }
    monitors
        .get(node.monitor_index as usize)
        .or_else(|| {
            monitors
                .iter()
                .find(|monitor| monitor.primary && node.is_primary)
        })
        .or_else(|| monitors.first())
}

fn layout_point_to_screen(
    layout: &Layout,
    local: DeviceId,
    monitors: &[nc_platform::MonitorInfo],
    node_id: &DisplayNodeId,
    point: Point,
    use_work_area: bool,
) -> Option<(i32, i32)> {
    let node = layout.node(node_id)?;
    if node.device_id != local || !node.is_local || !layout.is_active(&node.id) {
        return None;
    }
    let monitor = monitor_for_local_node(node, monitors)?;
    let usable = if use_work_area {
        monitor.work_area
    } else {
        monitor.bounds
    };
    let fraction_x =
        ((point.x - node.origin.x) / node.resolution.width.max(1) as f64).clamp(0.0, 1.0);
    let fraction_y =
        ((point.y - node.origin.y) / node.resolution.height.max(1) as f64).clamp(0.0, 1.0);
    Some((
        (usable.x as f64 + fraction_x * usable.width as f64).round() as i32,
        (usable.y as f64 + fraction_y * usable.height as f64).round() as i32,
    ))
}

fn mapped_entry_point(layout: &Layout, crossing: &EdgeCrossing, target: &DisplayNode) -> Point {
    let point = layout
        .map_edge_crossing(crossing)
        .unwrap_or_else(|| target.center());
    Point::new(point.x - target.origin.x, point.y - target.origin.y)
}

fn remote_cursor_offset(layout: &Layout, peer: DeviceId, target: &DisplayNode) -> (f64, f64) {
    let primary = layout
        .nodes
        .iter()
        .find(|node| {
            !node.is_local
                && node.device_id == peer
                && layout.is_active(&node.id)
                && node.is_primary
        })
        .or_else(|| {
            layout
                .nodes
                .iter()
                .find(|node| !node.is_local && node.device_id == peer && layout.is_active(&node.id))
        });
    let Some(primary) = primary else {
        return (0.0, 0.0);
    };
    match (
        layout.remote_desktop_origins.get(&target.id),
        layout.remote_desktop_origins.get(&primary.id),
    ) {
        (Some(target_origin), Some(primary_origin)) => (
            target_origin.x - primary_origin.x,
            target_origin.y - primary_origin.y,
        ),
        _ => (
            target.origin.x - primary.origin.x,
            target.origin.y - primary.origin.y,
        ),
    }
}

fn clamp_to_stage(x: f64, y: f64, stage: Option<(f64, f64, f64, f64)>) -> Option<(f64, f64)> {
    let (stage_x, stage_y, width, height) = stage?;
    let max_x = (stage_x + width - 1.0).max(stage_x);
    let max_y = (stage_y + height - 1.0).max(stage_y);
    let clamped_x = x.clamp(stage_x, max_x);
    let clamped_y = y.clamp(stage_y, max_y);
    ((clamped_x - x).abs() >= 0.5 || (clamped_y - y).abs() >= 0.5).then_some((clamped_x, clamped_y))
}

fn map_absolute_stage_event(state: &Arc<AppState>, event: &InputEvent) -> Option<InputEvent> {
    let (x, y) = match event {
        InputEvent::MouseMove { x, y, .. }
        | InputEvent::MouseButton { x, y, .. }
        | InputEvent::MouseWheel { x, y, .. } => (*x as f64, *y as f64),
        _ => return None,
    };
    let is_move = matches!(event, InputEvent::MouseMove { .. });
    let (mapped, offset, should_emit, return_arm, clamp_to) = {
        let mut input = state.input.lock();
        let mapped = input.map_absolute_stage_pointer(x, y)?;
        let now = crate::notifications::now_ms();
        let should_emit = now.saturating_sub(input.last_cursor_emit_ms) >= CURSOR_EMIT_INTERVAL_MS;
        if should_emit {
            input.last_cursor_emit_ms = now;
        }
        let return_arm = if is_move {
            input.update_absolute_edge(x, y, now)
        } else {
            None
        };
        let clamp_to = clamp_to_stage(x, y, input.stage_bounds);
        (
            mapped,
            input.remote_cursor_offset,
            should_emit,
            return_arm,
            clamp_to,
        )
    };
    if let Some((clamped_x, clamped_y)) = clamp_to {
        let _ =
            nc_platform::set_cursor_position(clamped_x.round() as i32, clamped_y.round() as i32);
    }
    if let Some((direction, generation)) = return_arm {
        spawn_absolute_edge_timer(state, direction, generation);
    }
    if should_emit {
        emit_remote_cursor(state);
    }
    let out_x = (mapped.0 + offset.0).round() as i32;
    let out_y = (mapped.1 + offset.1).round() as i32;
    Some(match event {
        InputEvent::MouseMove {
            delta_x,
            delta_y,
            modifiers,
            ..
        } => InputEvent::MouseMove {
            x: out_x,
            y: out_y,
            delta_x: *delta_x,
            delta_y: *delta_y,
            modifiers: *modifiers,
        },
        InputEvent::MouseButton {
            state: button_state,
            button,
            modifiers,
            ..
        } => InputEvent::MouseButton {
            state: *button_state,
            button: *button,
            x: out_x,
            y: out_y,
            modifiers: *modifiers,
        },
        InputEvent::MouseWheel {
            delta_x,
            delta_y,
            modifiers,
            ..
        } => InputEvent::MouseWheel {
            delta_x: *delta_x,
            delta_y: *delta_y,
            x: out_x,
            y: out_y,
            modifiers: *modifiers,
        },
        _ => return None,
    })
}

fn spawn_absolute_edge_timer(state: &Arc<AppState>, direction: Direction, generation: u64) {
    let state = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ABSOLUTE_EDGE_DWELL_MS)).await;
        let (still_armed, node, cursor) = {
            let input = state.input.lock();
            (
                input.absolute_pointer
                    && input.absolute_edge_armed
                    && input.absolute_edge_generation == generation
                    && input.controlling_remote,
                input.remote_node.clone(),
                input.virtual_cursor,
            )
        };
        let (Some(node), Some(cursor)) = (node, cursor) else {
            return;
        };
        if !still_armed || state.secure_desktop.load(Ordering::SeqCst) {
            return;
        }
        tracing::info!(?direction, from = %node, "绝对指针到达边缘，按布局切换");
        let handled = follow_remote_edge(&state, &node, direction, cursor, now_us(), true).await;
        if !handled {
            show_stage_edge_hint(&state, &node, direction);
        }
    });
}

fn show_stage_edge_hint(state: &Arc<AppState>, node: &DisplayNodeId, direction: Direction) {
    let snapshot = state.snapshot();
    if snapshot.mode != Mode::Aggregation || !snapshot.stage_visible {
        return;
    }
    let Some(target) = state.layout.read().node(node).cloned() else {
        return;
    };
    ipc::emit(
        &state.app,
        ipc::EVENT_STAGE_EDGE_HINT,
        ipc::StageEdgeHintView {
            device: target.device_name,
            display: target.display_name.trim().to_string(),
            direction: crate::osd::direction_key(direction).to_string(),
        },
    );
}

fn map_pointer_to_virtual_cursor(state: &AppState, event: &InputEvent) -> Option<InputEvent> {
    let (position, offset) = {
        let input = state.input.lock();
        (input.virtual_cursor?, input.remote_cursor_offset)
    };
    let x = (position.0 + offset.0).round() as i32;
    let y = (position.1 + offset.1).round() as i32;
    Some(match event {
        InputEvent::MouseButton {
            state: button_state,
            button,
            modifiers,
            ..
        } => InputEvent::MouseButton {
            state: *button_state,
            button: *button,
            x,
            y,
            modifiers: *modifiers,
        },
        InputEvent::MouseWheel {
            delta_x,
            delta_y,
            modifiers,
            ..
        } => InputEvent::MouseWheel {
            delta_x: *delta_x,
            delta_y: *delta_y,
            x,
            y,
            modifiers: *modifiers,
        },
        InputEvent::Key { .. } => event.clone(),
        _ => return None,
    })
}

fn warp_local_cursor_inside(
    state: &AppState,
    local_edge: Direction,
    preferred: Option<&DisplayNodeId>,
    along_fraction: Option<f64>,
) -> Option<DisplayNodeId> {
    let use_work_area = state.settings.read().edge_inset;
    let layout = state.layout.read().clone();
    let monitors = state.monitors.read().clone();
    let local = state.identity.device_id();
    let cursor = nc_platform::cursor_position()
        .ok()
        .map(|(x, y)| Point::new(x as f64, y as f64));
    let mapped = cursor.and_then(|cursor| {
        local_cursor_in_layout(&layout, local, &monitors, cursor, use_work_area)
    });
    let node = preferred
        .and_then(|node_id| {
            layout
                .node(node_id)
                .filter(|node| node.is_local && layout.is_active(&node.id))
        })
        .or_else(|| mapped.as_ref().and_then(|(node_id, _)| layout.node(node_id)))
        .or_else(|| {
            layout.nodes.iter().find(|node| {
                node.device_id == local
                    && node.is_local
                    && layout.is_active(&node.id)
                    && node.is_primary
            })
        })
        .or_else(|| {
            layout.nodes.iter().find(|node| {
                node.device_id == local && node.is_local && layout.is_active(&node.id)
            })
        })
        .cloned();
    let node = node?;
    let base = if let Some(fraction) = along_fraction {
        let fraction = fraction.clamp(0.0, 1.0);
        let center = node.center();
        match local_edge {
            Direction::Left | Direction::Right => Point::new(
                center.x,
                node.origin.y + fraction * node.resolution.height as f64,
            ),
            Direction::Up | Direction::Down => Point::new(
                node.origin.x + fraction * node.resolution.width as f64,
                center.y,
            ),
        }
    } else {
        mapped
            .as_ref()
            .filter(|(node_id, _)| node_id == &node.id)
            .map(|(_, point)| *point)
            .unwrap_or_else(|| node.center())
    };
    let point = warp_point_inside(node.bounds(), base, local_edge);
    tracing::info!(
        node = %node.id,
        ?local_edge,
        ?along_fraction,
        x = point.x,
        y = point.y,
        "本机光标归位"
    );
    if let Some((screen_x, screen_y)) =
        layout_point_to_screen(&layout, local, &monitors, &node.id, point, use_work_area)
    {
        let _ = nc_platform::set_cursor_position(screen_x, screen_y);
    } else {
        let _ = nc_platform::set_cursor_position(point.x.round() as i32, point.y.round() as i32);
    }
    Some(node.id.clone())
}

fn warp_along_fraction(
    edge: Direction,
    cursor: Option<(f64, f64)>,
    bounds: Option<(f64, f64)>,
) -> Option<f64> {
    let (width, height) = bounds?;
    let (x, y) = cursor?;
    match edge {
        Direction::Left | Direction::Right => {
            (height > 0.0).then(|| (y / height).clamp(0.0, 1.0))
        }
        Direction::Up | Direction::Down => {
            (width > 0.0).then(|| (x / width).clamp(0.0, 1.0))
        }
    }
}

fn warp_point_inside(bounds: Rect, base: Point, local_edge: Direction) -> Point {
    let x = base.x.clamp(bounds.x, bounds.right());
    let y = base.y.clamp(bounds.y, bounds.bottom());
    match local_edge {
        Direction::Right => Point::new(bounds.right() - 12.0, y),
        Direction::Left => Point::new(bounds.x + 12.0, y),
        Direction::Down => Point::new(x, bounds.bottom() - 12.0),
        Direction::Up => Point::new(x, bounds.y + 12.0),
    }
}

fn local_entry_edge(remote_exit: Direction) -> Direction {
    remote_exit.opposite()
}

fn forward_input(state: &AppState, event: InputEvent, timestamp_us: u64) -> bool {
    let envelope = InputEnvelope {
        seq: state.input_seq.fetch_add(1, Ordering::SeqCst) + 1,
        timestamp_us,
        event,
    };
    send_control(state, ControlMessage::Input { envelope })
}

fn now_us() -> u64 {
    crate::notifications::now_ms().saturating_mul(1000)
}

async fn emergency_release(state: &Arc<AppState>) {
    let was_remote = state.input.lock().controlling_remote
        || !matches!(state.snapshot().input_owner, nc_core::InputOwner::Local);
    set_switching_target(state, false);
    reset_input_runtime(state);
    send_control(state, ControlMessage::ReleaseAll);
    dispatch_session(state, SessionEvent::EmergencyRelease).await;
    if was_remote {
        show_edge_release_osd(state, None, true);
    }
}

fn input_service_available() -> bool {
    #[cfg(windows)]
    {
        crate::input_service::available()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn handle_file_drop_broker_event(state: &Arc<AppState>, event: BrokerEvent) {
    match event {
        BrokerEvent::Enter { count, x, y } => {
            let node = crate::file_drop::node_at_position(state, x, y);
            crate::file_drop::set_broker_hover(state, node, Some(count));
        }
        BrokerEvent::Over { x, y } => {
            let node = crate::file_drop::node_at_position(state, x, y);
            crate::file_drop::set_broker_hover(state, node, None);
        }
        BrokerEvent::Leave => {
            crate::file_drop::set_broker_hover(state, None, Some(0));
        }
        BrokerEvent::Drop { paths, x, y } => {
            let node_id = crate::file_drop::resolve_drop_node(state, x, y);
            tracing::info!(node = ?node_id, files = paths.len(), x, y, "拖放代理落点");
            crate::file_drop::hide(state);
            if !state.settings.read().file_transfer_enabled || paths.is_empty() {
                return;
            }
            let Some(node_id) = node_id else {
                return;
            };
            let Some(target) = crate::file_drop::drop_device(state, &node_id) else {
                return;
            };
            let paths: Vec<std::path::PathBuf> =
                paths.into_iter().map(std::path::PathBuf::from).collect();
            let state = Arc::clone(state);
            tauri::async_runtime::spawn(async move {
                send_files(&state, paths, Some(target)).await;
            });
        }
        BrokerEvent::Error { message } => {
            tracing::warn!(%message, "文件拖放代理报告错误");
        }
        BrokerEvent::Disconnected { reason } => {
            tracing::warn!(%reason, "文件拖放代理已断开");
            notify_file_drop_broker_issue(state, false);
        }
        BrokerEvent::Unavailable { reason } => {
            tracing::warn!(%reason, "文件拖放代理不可用");
            notify_file_drop_broker_issue(state, true);
        }
    }
}

fn notify_file_drop_broker_issue(state: &Arc<AppState>, unavailable: bool) {
    static LAST_NOTICE: std::sync::OnceLock<parking_lot::Mutex<Option<Instant>>> =
        std::sync::OnceLock::new();
    {
        let mut last = LAST_NOTICE
            .get_or_init(|| parking_lot::Mutex::new(None))
            .lock();
        let now = Instant::now();
        if let Some(previous) = *last {
            if now.duration_since(previous) < Duration::from_secs(10) {
                return;
            }
        }
        *last = Some(now);
    }
    let language = state.language();
    let (title, body) = if unavailable {
        i18n::file_drop_broker_unavailable(language)
    } else {
        i18n::file_drop_broker_disconnected(language)
    };
    add_notification(
        state,
        Severity::Warning,
        NotificationCategory::System,
        title,
        body,
        None,
    );
}

pub(crate) fn notify_autostart_failed(state: &Arc<AppState>, detail: &str) {
    let language = state.language();
    let (title, body) = i18n::autostart_task_failed(language);
    add_notification(
        state,
        Severity::Warning,
        NotificationCategory::System,
        title,
        format!("{body}\n{detail}"),
        None,
    );
}

async fn check_secure_desktop(state: &Arc<AppState>) {
    let active = nc_platform::secure_desktop_active();
    let target_active = *state.session_role.read() == Some(SessionRole::Target) && !active;
    if let Err(error) = nc_platform::set_idle_lock_prevention(target_active) {
        tracing::warn!(%error, "更新闲置锁屏防护状态失败");
    }
    let was_active = state.secure_desktop.swap(active, Ordering::SeqCst);
    if active == was_active {
        return;
    }
    let language = state.language();
    if active {
        let service_mode =
            *state.session_role.read() == Some(SessionRole::Target) && input_service_available();
        state
            .secure_desktop_service
            .store(service_mode, Ordering::SeqCst);
        if service_mode {
            state.input.lock().release_all();
            let (title, body) = i18n::secure_desktop_service_active(language);
            send_remote_notice_marked(
                state,
                Severity::Info,
                NotificationCategory::System,
                title.clone(),
                body.clone(),
                "secure_desktop_active",
            );
            add_priority_notification(
                state,
                Severity::Info,
                NotificationCategory::System,
                title,
                body,
                None,
            );
            return;
        }
        reset_input_runtime(state);
        if matches!(
            state.snapshot().input_owner,
            nc_core::InputOwner::Remote { .. }
        ) {
            send_control(state, ControlMessage::ReleaseAll);
            dispatch_session(state, SessionEvent::ReleaseAll).await;
        }
        let (title, body) = i18n::secure_desktop_paused(language);
        send_remote_notice_marked(
            state,
            Severity::Warning,
            NotificationCategory::System,
            title.clone(),
            body.clone(),
            "secure_desktop_active",
        );
        add_priority_notification(
            state,
            Severity::Warning,
            NotificationCategory::System,
            title,
            body,
            None,
        );
        return;
    }
    let last_stream_request = *state.last_stream_request.lock();
    if *state.session_role.read() == Some(SessionRole::Target) {
        if let Some(request) = last_stream_request {
            tracing::info!("安全桌面结束，重启被控端采集流");
            handle_stream_request(state, request).await;
        }
    }
    if state.secure_desktop_service.swap(false, Ordering::SeqCst) {
        let (title, body) = i18n::secure_desktop_service_resumed(language);
        send_remote_notice_marked(
            state,
            Severity::Info,
            NotificationCategory::System,
            title.clone(),
            body.clone(),
            "secure_desktop_inactive",
        );
        add_notification(
            state,
            Severity::Info,
            NotificationCategory::System,
            title,
            body,
            None,
        );
        return;
    }
    let (title, body) = i18n::secure_desktop_resumed(language);
    send_remote_notice_marked(
        state,
        Severity::Info,
        NotificationCategory::System,
        title.clone(),
        body.clone(),
        "secure_desktop_inactive",
    );
    add_notification(
        state,
        Severity::Info,
        NotificationCategory::System,
        title,
        body,
        None,
    );
}

fn send_remote_notice_marked(
    state: &AppState,
    severity: Severity,
    category: NotificationCategory,
    title: String,
    body: String,
    marker: &str,
) {
    if *state.session_role.read() != Some(SessionRole::Target) {
        send_remote_notice(state, severity, category, &title, &body);
        return;
    }
    send_control(
        state,
        ControlMessage::Notify {
            notification: AppNotification {
                id: 0,
                severity,
                category,
                title: title.clone(),
                body,
                action: Some(NotificationAction {
                    kind: NotificationActionKind::OpenControlPanel,
                    label: title,
                    payload: Some(marker.to_string()),
                }),
                timestamp_ms: crate::notifications::now_ms(),
                read: false,
            },
        },
    );
}

fn send_remote_notice(
    state: &AppState,
    severity: Severity,
    category: NotificationCategory,
    title: &str,
    body: &str,
) {
    send_control(
        state,
        ControlMessage::Notify {
            notification: AppNotification {
                id: 0,
                severity,
                category,
                title: title.to_string(),
                body: body.to_string(),
                action: None,
                timestamp_ms: crate::notifications::now_ms(),
                read: false,
            },
        },
    );
}

fn manual_control_allowed(role: Option<SessionRole>, remote_input_released: bool) -> bool {
    role != Some(SessionRole::Target) || remote_input_released
}

fn local_edge_takeover(role: Option<SessionRole>, remote_input_released: bool) -> bool {
    role == Some(SessionRole::Target) && remote_input_released
}

fn should_switch_session(
    previous: Option<DeviceId>,
    target: DeviceId,
    role: Option<SessionRole>,
    remote_input_released: bool,
) -> bool {
    previous != Some(target) || local_edge_takeover(role, remote_input_released)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRequestAction {
    Proceed,
    Resend,
    Override,
}

fn pending_request_action(pending: Option<DeviceId>, target: DeviceId) -> PendingRequestAction {
    match pending {
        None => PendingRequestAction::Proceed,
        Some(pending) if pending == target => PendingRequestAction::Resend,
        Some(_) => PendingRequestAction::Override,
    }
}

fn stale_host_role(role: Option<SessionRole>, has_session_peer: bool) -> bool {
    role == Some(SessionRole::Host) && !has_session_peer
}

fn group_host_blocks(host: Option<DeviceId>, local: DeviceId) -> bool {
    host.is_some_and(|host| host != local)
}

fn clear_released_host_lock(state: &AppState, role: Option<SessionRole>, released: bool) {
    if !local_edge_takeover(role, released) {
        return;
    }
    let Some(peer) = state.session_peer_id() else {
        return;
    };
    if *state.group_host.read() == Some(peer) {
        set_group_host(state, None);
    }
}

fn set_switching_target(state: &AppState, active: bool) {
    if state.switching_target.swap(active, Ordering::SeqCst) != active {
        if active {
            tracing::info!("目标切换开始");
        } else {
            tracing::info!("目标切换结束");
        }
        ipc::emit(
            &state.app,
            ipc::EVENT_STAGE_SWITCHING,
            ipc::StageSwitchingView { active },
        );
    }
}

async fn check_pending_watchdog(state: &Arc<AppState>) {
    check_pending_session_watchdog(state).await;
    check_pending_takeover_watchdog(state);
}

async fn check_pending_session_watchdog(state: &Arc<AppState>) {
    let pending_target = *state.pending_session.read();
    let Some(target) = pending_target else {
        return;
    };
    if state.snapshot().state == nc_core::SessionState::Reconnecting {
        return;
    }
    let elapsed = {
        let started = state.pending_session_started.lock();
        started.as_ref().map(Instant::elapsed).unwrap_or_default()
    };
    if elapsed < PENDING_SESSION_TIMEOUT {
        return;
    }
    let retries = state.pending_session_retries.load(Ordering::SeqCst);
    if retries < PENDING_SESSION_MAX_RETRIES {
        state
            .pending_session_retries
            .store(retries.saturating_add(1), Ordering::SeqCst);
        *state.pending_session_started.lock() = Some(Instant::now());
        tracing::warn!(%target, attempt = retries + 1, "待处理目标超时，重发会话打开");
        let linked = state
            .links
            .lock()
            .get(&target)
            .is_some_and(|link| link.connected.load(Ordering::SeqCst));
        if linked {
            send_session_open(state, target);
        } else {
            ensure_group_link(state, target, PairingIntent::Trusted, true).await;
        }
        return;
    }
    tracing::warn!(%target, "待处理目标超时，放弃并回到本机");
    abort_pending_session(state, "等待目标响应超时", true).await;
}

fn check_pending_takeover_watchdog(state: &Arc<AppState>) {
    let expired = {
        let pending = state.pending_takeover.lock();
        pending
            .as_ref()
            .filter(|request| request.started.elapsed() >= PENDING_TAKEOVER_TIMEOUT)
            .map(|request| (request.host, request.target))
    };
    let Some((host, target)) = expired else {
        return;
    };
    tracing::warn!(%host, %target, "等待主机释放超时");
    clear_pending_takeover(state);
    let language = state.language();
    let host_name = device_display_name(state, host);
    let (title, body) = i18n::host_takeover_timed_out(language, &host_name);
    add_notification(
        state,
        Severity::Warning,
        NotificationCategory::Connection,
        title,
        body,
        Some(NotificationAction {
            kind: NotificationActionKind::RetryConnection,
            label: i18n::retry_action(language),
            payload: Some(target.to_hex()),
        }),
    );
}

fn device_display_name(state: &AppState, device: DeviceId) -> String {
    state
        .link_peer(device)
        .map(|peer| peer.name)
        .or_else(|| {
            state
                .devices
                .read()
                .get(&device)
                .map(|peer| peer.info.name.clone())
        })
        .unwrap_or_else(|| i18n::generic_device(state.language()))
}

async fn abort_pending_session(state: &Arc<AppState>, reason: &str, notify: bool) {
    let pending_target = *state.pending_session.read();
    let Some(target) = pending_target else {
        return;
    };
    tracing::warn!(%target, %reason, "放弃待处理控制");
    let linked = state
        .links
        .lock()
        .get(&target)
        .is_some_and(|link| link.connected.load(Ordering::SeqCst));
    if linked {
        send_control_to(state, target, ControlMessage::SessionClose);
    }
    if let Some(handle) = state.reconnect_task.lock().take() {
        handle.abort();
    }
    state.reconnect_attempt.store(0, Ordering::SeqCst);
    set_pending_session(state, None);
    *state.pending_connect.write() = None;
    *state.pending_remote_entry.lock() = None;
    set_switching_target(state, false);
    reset_input_runtime(state);
    let was_host = *state.session_role.read() == Some(SessionRole::Host);
    *state.session_peer.write() = None;
    *state.session_role.write() = None;
    *state.session_peer_mode.write() = None;
    state.media.clear_session();
    state.media.stop_all_outgoing().await;
    match state.snapshot().state {
        nc_core::SessionState::Reconnecting => {
            dispatch_session(state, SessionEvent::UserAbort).await;
        }
        session_state if session_state.is_active() => {
            dispatch_session(state, SessionEvent::RemoteSessionClosed).await;
        }
        _ => {
            dispatch_session(
                state,
                SessionEvent::ConnectionLost {
                    reason: reason.to_string(),
                },
            )
            .await;
        }
    }
    if was_host || *state.group_host.read() == Some(state.identity.device_id()) {
        broadcast_host_status(state, false);
    }
    if notify {
        let language = state.language();
        let (title, body) = i18n::switch_timed_out(language);
        add_notification(
            state,
            Severity::Warning,
            NotificationCategory::Connection,
            title,
            body,
            Some(NotificationAction {
                kind: NotificationActionKind::RetryConnection,
                label: i18n::retry_action(language),
                payload: Some(target.to_hex()),
            }),
        );
    }
}

fn broadcast_host_status(state: &AppState, active: bool) {
    let local = state.identity.device_id();
    if active {
        set_group_host(state, Some(local));
    } else if *state.group_host.read() == Some(local) {
        set_group_host(state, None);
    }
    let devices: Vec<DeviceId> = state.links.lock().keys().copied().collect();
    for device in devices {
        send_control_to(
            state,
            device,
            ControlMessage::HostStatus {
                host: local,
                active,
            },
        );
    }
}

fn apply_remote_host_status(
    state: &AppState,
    from: DeviceId,
    host: DeviceId,
    active: bool,
) -> bool {
    if host != from {
        tracing::warn!(%from, %host, "忽略非主机来源的主机状态");
        return false;
    }
    let mut release_confirmed = false;
    {
        let mut current = state.group_host.write();
        if active {
            if current.is_some_and(|value| value != host) {
                tracing::warn!(%host, "组内已有其他活动主机，保留现有状态");
                return false;
            }
            if *current != Some(host) {
                *current = Some(host);
                release_confirmed = true;
            }
        } else if *current == Some(host) {
            *current = None;
            release_confirmed = true;
        }
    }
    if release_confirmed {
        emit_group_status(state);
    }
    if !active {
        finish_pending_takeover(state, from);
    }
    true
}

fn finish_pending_takeover(state: &AppState, host: DeviceId) {
    let target = {
        let mut pending = state.pending_takeover.lock();
        match pending.as_ref() {
            Some(request) if request.host == host => pending.take().map(|request| request.target),
            _ => None,
        }
    };
    let Some(target) = target else {
        return;
    };
    tracing::info!(%host, %target, "主机已释放，继续接管");
    emit_group_status(state);
    let _ = state.actions.send(UiAction::ConnectDevice(target));
}

fn local_flow_allowed(
    state: &AppState,
    snapshot: &SessionSnapshot,
    role: Option<SessionRole>,
    remote_input_released: bool,
) -> bool {
    if group_host_blocks(*state.group_host.read(), state.identity.device_id()) {
        return false;
    }
    let has_link = state
        .links
        .lock()
        .values()
        .any(|link| link.connected.load(Ordering::SeqCst));
    local_flow_allowed_for(
        role,
        remote_input_released,
        snapshot.state,
        snapshot.input_owner.is_local(),
        state.user_disconnect.load(Ordering::SeqCst),
        state.pending_session.read().is_some(),
        has_link,
    )
}

fn local_flow_allowed_for(
    role: Option<SessionRole>,
    remote_input_released: bool,
    state: nc_core::SessionState,
    input_local: bool,
    user_disconnect: bool,
    pending_session: bool,
    has_link: bool,
) -> bool {
    if user_disconnect || pending_session || !input_local {
        return false;
    }
    if role == Some(SessionRole::Target) {
        return remote_input_released;
    }
    if state.is_active() {
        return true;
    }
    matches!(
        state,
        nc_core::SessionState::Idle
            | nc_core::SessionState::Discovering
            | nc_core::SessionState::Error
            | nc_core::SessionState::Connected
    ) && has_link
}

fn should_watchdog_release(
    controlling_remote: bool,
    input_owner: &nc_core::InputOwner,
    capture_running: bool,
) -> bool {
    controlling_remote
        && matches!(input_owner, nc_core::InputOwner::Remote { .. })
        && !capture_running
}

async fn check_input_watchdog(state: &Arc<AppState>) {
    let snapshot = state.snapshot();
    let (controlling_remote, capture_running) = {
        let input = state.input.lock();
        (
            input.controlling_remote,
            input.capture.as_ref().is_some_and(|capture| capture.is_running()),
        )
    };
    if should_watchdog_release(controlling_remote, &snapshot.input_owner, capture_running) {
        tracing::warn!("输入捕获已停止，自动释放远端输入");
        emergency_release(state).await;
    }
}

fn edge_direction_at_cursor(
    state: &AppState,
    x: i32,
    y: i32,
) -> Option<(DisplayNodeId, Direction)> {
    let use_work_area = state.settings.read().edge_inset;
    let layout = state.layout.read();
    let monitors = state.monitors.read();
    let local = state.identity.device_id();
    let (node_id, point) = local_cursor_in_layout(
        &layout,
        local,
        &monitors,
        Point::new(x as f64, y as f64),
        use_work_area,
    )?;
    let node = layout.node(&node_id)?;
    let bounds = node.bounds();
    let edge = 2.0;
    let direction = if (point.x - bounds.x).abs() <= edge {
        Direction::Left
    } else if (bounds.right() - point.x).abs() <= edge {
        Direction::Right
    } else if (point.y - bounds.y).abs() <= edge {
        Direction::Up
    } else if (bounds.bottom() - point.y).abs() <= edge {
        Direction::Down
    } else {
        return None;
    };
    Some((node_id, direction))
}

fn local_has_no_peer_on(state: &AppState, node_id: &DisplayNodeId, direction: Direction) -> bool {
    let layout = state.layout.read().clone();
    let local = state.identity.device_id();
    layout
        .neighbor_id(node_id, direction)
        .map(|neighbor| {
            layout
                .node(&neighbor)
                .map(|node| node.device_id == local)
                .unwrap_or(true)
        })
        .unwrap_or(true)
}

fn local_display_nodes(state: &AppState) -> Vec<DisplayNode> {
    let device_id = state.identity.device_id();
    let device_name = state.settings.read().device_name.clone();
    let language = state.language();
    let monitors = nc_platform::monitors().unwrap_or_default();
    let total = monitors.len();
    let mut nodes = Vec::with_capacity(total.max(1));
    for (index, monitor) in monitors.into_iter().enumerate() {
        let monitor_name = monitor.name.trim();
        let display_name = if monitor_name.is_empty() {
            i18n::screen_label(language, index)
        } else {
            monitor_name.to_string()
        };
        nodes.push(DisplayNode {
            id: DisplayNodeId::new(&device_id, index as u32),
            device_id,
            device_name: device_name.clone(),
            display_name,
            monitor_index: index as u32,
            resolution: nc_core::Size::new(monitor.bounds.width, monitor.bounds.height),
            dpi: monitor.dpi as f32,
            physical: monitor.display_physical(),
            origin: Point::new(monitor.bounds.x as f64, monitor.bounds.y as f64),
            is_local: true,
            is_primary: monitor.primary,
        });
    }
    if nodes.is_empty() {
        nodes.push(DisplayNode {
            id: DisplayNodeId::new(&device_id, 0),
            device_id,
            device_name,
            display_name: i18n::screen_label(language, 0),
            monitor_index: 0,
            resolution: nc_core::Size::new(1920, 1080),
            dpi: 96.0,
            physical: nc_core::PhysicalSize::default(),
            origin: Point::new(0.0, 0.0),
            is_local: true,
            is_primary: true,
        });
    }
    let overrides = state.layout.read().orientation_overrides.clone();
    for node in &mut nodes {
        if let Some(orientation) = overrides.get(&node.id).copied() {
            node.resolution = orientation.apply(node.resolution);
            node.physical = orientation.apply_physical(node.physical);
        }
    }
    nodes
}

fn deactivate_device_nodes(state: &Arc<AppState>, device: DeviceId) {
    let stored = {
        let mut layout = state.layout.write();
        let ids: Vec<DisplayNodeId> = layout
            .nodes
            .iter()
            .filter(|node| node.device_id == device && !node.is_local)
            .map(|node| node.id.clone())
            .collect();
        let mut changed = false;
        for id in ids {
            if layout.active_node_ids.remove(&id) {
                changed = true;
            }
        }
        if !changed {
            return;
        }
        layout.revision = layout.revision.saturating_add(1);
        layout.clone()
    };
    update_layout(state, stored);
}

fn merge_remote_topology(state: &Arc<AppState>, nodes: Vec<DisplayNode>, source: DeviceId) {
    let local_id = state.identity.device_id();
    let peer = state.link_peer(source);
    let language = state.language();
    let stored = {
        let mut layout = state.layout.write();
        if !merge_remote_nodes(&mut layout, local_id, source, peer.as_ref(), language, nodes) {
            return;
        }
        layout.revision = layout.revision.saturating_add(1);
        layout.clone()
    };
    update_layout(state, stored);
}

fn merge_remote_nodes(
    layout: &mut Layout,
    local_id: DeviceId,
    source: DeviceId,
    peer: Option<&DeviceInfo>,
    language: nc_core::Language,
    nodes: Vec<DisplayNode>,
) -> bool {
    let mut remote_nodes: Vec<DisplayNode> = nodes
        .into_iter()
        .filter(|node| {
            node.device_id != local_id
                && node.device_id == source
                && node.id.device_id() == Some(source)
        })
        .collect();

    let mut incoming_devices: std::collections::HashSet<DeviceId> =
        remote_nodes.iter().map(|node| node.device_id).collect();
    if incoming_devices.is_empty() {
        if let Some(peer) = peer {
            incoming_devices.insert(peer.id);
        } else {
            return false;
        }
    }

    let peer_name = peer.map(|peer| peer.name.clone());
    let mut changed = false;
    let retired: Vec<DisplayNodeId> = layout
        .nodes
        .iter()
        .filter(|node| !node.is_local && incoming_devices.contains(&node.device_id))
        .map(|node| node.id.clone())
        .collect();
    for id in retired {
        if layout.active_node_ids.remove(&id) {
            changed = true;
        }
    }
    let overrides = layout.orientation_overrides.clone();
    for remote in &mut remote_nodes {
        if let Some(orientation) = overrides.get(&remote.id).copied() {
            remote.resolution = orientation.apply(remote.resolution);
            remote.physical = orientation.apply_physical(remote.physical);
        }
        remote.is_local = false;
        if remote.device_name.trim().is_empty() {
            if let Some(name) = &peer_name {
                remote.device_name = name.clone();
            }
        }
        if remote.display_name.trim().is_empty() {
            remote.display_name = i18n::screen_label(language, remote.monitor_index as usize);
        }
    }

    for remote in &remote_nodes {
        if remote.origin.x.is_finite() && remote.origin.y.is_finite() {
            layout
                .remote_desktop_origins
                .insert(remote.id.clone(), remote.origin);
        }
    }

    let existing_origins: std::collections::HashMap<DisplayNodeId, Point> = layout
        .nodes
        .iter()
        .filter(|node| !node.is_local && incoming_devices.contains(&node.device_id))
        .map(|node| (node.id.clone(), node.origin))
        .collect();

    let mut resized = std::collections::HashSet::new();
    for remote in &remote_nodes {
        if let Some(existing) = layout
            .nodes
            .iter_mut()
            .find(|existing| existing.id == remote.id)
        {
            let resolution_changed = existing.resize_centered(remote.resolution);
            if resolution_changed {
                resized.insert(existing.id.clone());
            }
            if existing.device_name != remote.device_name
                || existing.display_name != remote.display_name
                || resolution_changed
                || existing.dpi != remote.dpi
                || existing.physical != remote.physical
                || existing.is_primary != remote.is_primary
            {
                existing.device_name = remote.device_name.clone();
                existing.display_name = remote.display_name.clone();
                existing.dpi = remote.dpi;
                existing.physical = remote.physical;
                existing.is_primary = remote.is_primary;
                changed = true;
            }
        }
        if layout.active_node_ids.insert(remote.id.clone()) {
            changed = true;
        }
    }
    if layout.resolve_overlaps(&resized) {
        changed = true;
    }

    let new_remote: Vec<DisplayNode> = remote_nodes
        .iter()
        .filter(|remote| !layout.nodes.iter().any(|existing| existing.id == remote.id))
        .cloned()
        .collect();
    if !new_remote.is_empty() {
        let mut by_device: std::collections::BTreeMap<DeviceId, Vec<DisplayNode>> =
            std::collections::BTreeMap::new();
        for remote in new_remote {
            by_device.entry(remote.device_id).or_default().push(remote);
        }
        let active_nodes: Vec<&DisplayNode> = layout
            .nodes
            .iter()
            .filter(|node| layout.is_active(&node.id))
            .collect();
        let mut placement_left = active_nodes
            .iter()
            .map(|node| node.bounds().right())
            .fold(f64::NEG_INFINITY, f64::max);
        placement_left = if placement_left.is_finite() {
            placement_left + nc_topology::LAYOUT_GAP_PX
        } else {
            0.0
        };
        let local_nodes: Vec<&DisplayNode> = active_nodes
            .iter()
            .copied()
            .filter(|node| node.is_local)
            .collect();
        let edge_nodes: Vec<&DisplayNode> = if local_nodes.is_empty() {
            Vec::new()
        } else {
            let local_right = local_nodes
                .iter()
                .map(|node| node.bounds().right())
                .fold(f64::NEG_INFINITY, f64::max);
            local_nodes
                .iter()
                .copied()
                .filter(|node| (node.bounds().right() - local_right).abs() < 0.5)
                .collect()
        };
        let anchor_nodes: &[&DisplayNode] = if !edge_nodes.is_empty() {
            &edge_nodes
        } else if !local_nodes.is_empty() {
            &local_nodes
        } else {
            &active_nodes
        };
        let anchor_center_y = if anchor_nodes.is_empty() {
            0.0
        } else {
            let top = anchor_nodes
                .iter()
                .map(|node| node.bounds().y)
                .fold(f64::MAX, f64::min);
            let bottom = anchor_nodes
                .iter()
                .map(|node| node.bounds().bottom())
                .fold(f64::MIN, f64::max);
            (top + bottom) / 2.0
        };
        for group in by_device.values_mut() {
            let anchor = remote_nodes
                .iter()
                .filter(|node| node.device_id == group[0].device_id)
                .find_map(|node| {
                    existing_origins
                        .get(&node.id)
                        .map(|origin| (*origin, node.origin))
                });
            if let Some((existing_origin, incoming_origin)) = anchor {
                let dx = existing_origin.x - incoming_origin.x;
                let dy = existing_origin.y - incoming_origin.y;
                for remote in group.iter_mut() {
                    remote.origin = Point::new(remote.origin.x + dx, remote.origin.y + dy);
                }
            } else {
                let block_left = group
                    .iter()
                    .map(|node| node.bounds().x)
                    .fold(f64::MAX, f64::min);
                let block_top = group
                    .iter()
                    .map(|node| node.bounds().y)
                    .fold(f64::MAX, f64::min);
                let block_bottom = group
                    .iter()
                    .map(|node| node.bounds().bottom())
                    .fold(f64::MIN, f64::max);
                let dx = placement_left - block_left;
                let dy = anchor_center_y - (block_top + block_bottom) / 2.0;
                for remote in group.iter_mut() {
                    remote.origin = Point::new(remote.origin.x + dx, remote.origin.y + dy);
                }
            }
            for remote in group.drain(..) {
                layout.active_node_ids.insert(remote.id.clone());
                layout.nodes.push(remote);
                changed = true;
            }
            let active_right = layout
                .nodes
                .iter()
                .filter(|node| layout.is_active(&node.id))
                .map(|node| node.bounds().right())
                .fold(f64::NEG_INFINITY, f64::max);
            if active_right.is_finite() {
                placement_left = active_right + nc_topology::LAYOUT_GAP_PX;
            }
        }
    }

    changed
}

fn rect_overlaps_active(layout: &Layout, origin: Point, width: u32, height: u32) -> bool {
    let rect = Rect::new(origin.x, origin.y, width as f64, height as f64);
    layout
        .nodes
        .iter()
        .filter(|node| layout.is_active(&node.id))
        .any(|node| {
            let bounds = node.bounds();
            rect.x < bounds.right()
                && bounds.x < rect.right()
                && rect.y < bounds.bottom()
                && bounds.y < rect.bottom()
        })
}

fn reconcile_local_nodes(
    layout: &mut Layout,
    local: DeviceId,
    monitors: &[nc_platform::MonitorInfo],
    device_name: &str,
    language: nc_core::Language,
) -> bool {
    if monitors.is_empty() {
        return false;
    }
    let mut changed = false;
    let mut present = vec![false; monitors.len()];
    let mut resized = std::collections::HashSet::new();
    let overrides = layout.orientation_overrides.clone();
    for node in &mut layout.nodes {
        if node.device_id != local || !node.is_local {
            continue;
        }
        let index = node.monitor_index as usize;
        let Some(monitor) = monitors.get(index) else {
            if layout.active_node_ids.remove(&node.id) {
                changed = true;
            }
            continue;
        };
        present[index] = true;
        let native_resolution = nc_core::Size::new(monitor.bounds.width, monitor.bounds.height);
        let native_physical = monitor.display_physical();
        let orientation = overrides.get(&node.id).copied();
        let resolution =
            orientation.map_or(native_resolution, |value| value.apply(native_resolution));
        let physical = orientation.map_or(native_physical, |value| {
            value.apply_physical(native_physical)
        });
        let display_name = if monitor.name.trim().is_empty() {
            i18n::screen_label(language, index)
        } else {
            monitor.name.trim().to_string()
        };
        let resolution_changed = node.resize_centered(resolution);
        if resolution_changed {
            resized.insert(node.id.clone());
        }
        if node.device_name != device_name
            || node.display_name != display_name
            || resolution_changed
            || node.dpi != monitor.dpi as f32
            || node.physical != physical
            || node.is_primary != monitor.primary
        {
            node.device_name = device_name.to_string();
            node.display_name = display_name;
            node.dpi = monitor.dpi as f32;
            node.physical = physical;
            node.is_primary = monitor.primary;
            changed = true;
        }
        if layout.active_node_ids.insert(node.id.clone()) {
            changed = true;
        }
    }
    if layout.resolve_overlaps(&resized) {
        changed = true;
    }

    let mut right = layout
        .nodes
        .iter()
        .filter(|node| node.device_id == local && node.is_local && layout.is_active(&node.id))
        .map(|node| node.bounds().right())
        .fold(f64::NEG_INFINITY, f64::max);
    let primary_center_y = layout
        .nodes
        .iter()
        .find(|node| {
            node.device_id == local
                && node.is_local
                && layout.is_active(&node.id)
                && node.is_primary
        })
        .or_else(|| {
            layout
                .nodes
                .iter()
                .find(|node| node.device_id == local && node.is_local && layout.is_active(&node.id))
        })
        .map(|node| node.center().y);
    for (index, monitor) in monitors.iter().enumerate() {
        if present[index] {
            continue;
        }
        let id = DisplayNodeId::new(&local, index as u32);
        if layout.nodes.iter().any(|node| node.id == id) {
            continue;
        }
        let native_resolution = nc_core::Size::new(monitor.bounds.width, monitor.bounds.height);
        let native_physical = monitor.display_physical();
        let resolution = overrides
            .get(&id)
            .copied()
            .map_or(native_resolution, |value| value.apply(native_resolution));
        let physical = overrides
            .get(&id)
            .copied()
            .map_or(native_physical, |value| {
                value.apply_physical(native_physical)
            });
        let height = resolution.height as f64;
        let mut origin = if right.is_finite() {
            Point::new(
                right,
                primary_center_y.unwrap_or(monitor.bounds.y as f64 + height / 2.0) - height / 2.0,
            )
        } else {
            Point::new(monitor.bounds.x as f64, monitor.bounds.y as f64)
        };
        if rect_overlaps_active(layout, origin, resolution.width, resolution.height) {
            let global_right = layout
                .nodes
                .iter()
                .filter(|node| layout.is_active(&node.id))
                .map(|node| node.bounds().right())
                .fold(f64::NEG_INFINITY, f64::max);
            if global_right.is_finite() {
                origin = Point::new(
                    global_right + nc_topology::LAYOUT_GAP_PX,
                    primary_center_y.unwrap_or(monitor.bounds.y as f64 + height / 2.0)
                        - height / 2.0,
                );
            }
        }
        right = origin.x + resolution.width as f64;
        let display_name = if monitor.name.trim().is_empty() {
            i18n::screen_label(language, index)
        } else {
            monitor.name.trim().to_string()
        };
        layout.active_node_ids.insert(id.clone());
        layout.nodes.push(DisplayNode {
            id,
            device_id: local,
            device_name: device_name.to_string(),
            display_name,
            monitor_index: index as u32,
            resolution,
            dpi: monitor.dpi as f32,
            physical,
            origin,
            is_local: true,
            is_primary: monitor.primary,
        });
        changed = true;
    }
    changed
}

fn reconcile_local_layout(state: &Arc<AppState>) -> bool {
    let monitors = state.monitors.read().clone();
    let settings = state.settings.read().clone();
    let local = state.identity.device_id();
    let stored = {
        let mut layout = state.layout.write();
        let changed = reconcile_local_nodes(
            &mut layout,
            local,
            &monitors,
            &settings.device_name,
            settings.language,
        );
        if !changed {
            return false;
        }
        layout.revision = layout.revision.saturating_add(1);
        layout.clone()
    };
    update_layout(state, stored);
    broadcast_topology(state);
    true
}

fn save_layout(state: &AppState, layout: &Layout) {
    if let Err(error) = layout.save(&state.paths.layout_file) {
        tracing::warn!(%error, "保存布局失败");
    }
}

pub fn load_initial_layout(state: &AppState) -> Layout {
    let monitors = nc_platform::monitors().unwrap_or_default();
    *state.monitors.write() = monitors.clone();
    match Layout::load(&state.paths.layout_file) {
        Ok(layout) if !layout.nodes.is_empty() => {
            let local = state.identity.device_id();
            let settings = state.settings.read().clone();
            let mut layout = layout;
            for node in &mut layout.nodes {
                if node.device_id == local {
                    node.is_local = true;
                }
            }
            let local_ids: std::collections::HashSet<DisplayNodeId> = layout
                .nodes
                .iter()
                .filter(|node| node.is_local)
                .map(|node| node.id.clone())
                .collect();
            let previous_active = layout.active_node_ids.len();
            layout.active_node_ids.retain(|id| local_ids.contains(id));
            let mut changed = layout.active_node_ids.len() != previous_active;
            changed |= reconcile_local_nodes(
                &mut layout,
                local,
                &monitors,
                &settings.device_name,
                settings.language,
            );
            changed |= layout.apply_orientation_overrides();
            if changed {
                layout.revision = layout.revision.saturating_add(1);
                save_layout(state, &layout);
            }
            layout
        }
        _ => Layout::new(local_display_nodes(state)),
    }
}

fn add_notification(
    state: &AppState,
    severity: Severity,
    category: NotificationCategory,
    title: String,
    body: String,
    action: Option<NotificationAction>,
) -> AppNotification {
    let notification = state
        .notifications
        .lock()
        .add(severity, category, title, body, action);
    ipc::emit(&state.app, ipc::EVENT_NOTIFICATION, notification.clone());
    let center = state.notifications.lock();
    ipc::emit(&state.app, ipc::EVENT_NOTIFICATIONS_CHANGED, center.list());
    let unread = center.unread_count();
    drop(center);
    update_tray(state, &state.snapshot());
    show_native_notification(state, &notification);
    let _ = unread;
    notification
}

fn add_priority_notification(
    state: &AppState,
    severity: Severity,
    category: NotificationCategory,
    title: String,
    body: String,
    action: Option<NotificationAction>,
) -> AppNotification {
    let notification = add_notification(state, severity, category, title, body, action);
    crate::window::request_attention(&state.app);
    notification
}

fn show_native_notification(state: &AppState, notification: &AppNotification) {
    if state.test_mode {
        return;
    }
    use tauri_plugin_notification::{Action, ActionType, NotificationExt};
    let mut builder = state
        .app
        .notification()
        .builder()
        .title(notification.title.clone())
        .body(notification.body.clone());
    if let Some(action) = &notification.action {
        let action_type_id = format!("nexuscat.notification.{}", notification.id);
        let action_type = ActionType::builder(&action_type_id)
            .actions(vec![
                Action::builder(action.kind.action_id(), action.label.clone()).build(),
            ])
            .build();
        if state
            .app
            .notification()
            .register_action_types(vec![action_type])
            .is_ok()
        {
            builder = builder.action_type_id(action_type_id);
        }
    }
    let _ = builder.show();
}

fn update_tray(state: &AppState, snapshot: &SessionSnapshot) {
    let unread = state.notifications.lock().unread_count();
    let language = state.language();
    let settings = state.settings.read().clone();
    if let Some(tray) = state.tray.read().as_ref() {
        tray.update(snapshot, unread, language, &settings);
    }
}

static METRICS_INFO_TICK: AtomicU64 = AtomicU64::new(0);

fn emit_metrics(state: &AppState) {
    let snapshot = state.metrics.snapshot();
    tracing::debug!(
        fps = snapshot.fps,
        capture_fps = snapshot.capture_fps,
        received_fps = snapshot.received_fps,
        presented_fps = snapshot.presented_fps,
        rtt_ms = snapshot.rtt_ms,
        loss_percent = snapshot.loss_percent,
        decode_ms = snapshot.decode_ms,
        present_ms = snapshot.present_ms,
        glass_p50_ms = ?snapshot.glass_to_glass_p50_ms,
        glass_p95_ms = ?snapshot.glass_to_glass_p95_ms,
        dropped_frames = snapshot.dropped_frames,
        "媒体指标"
    );
    let tick = METRICS_INFO_TICK.fetch_add(1, Ordering::Relaxed);
    if tick % 30 == 0 {
        let session = state.snapshot();
        if snapshot.glass_to_glass_p50_ms.is_some()
            && session.state == nc_core::SessionState::Aggregating
            && session.stage_visible
        {
            tracing::info!(
                fps = snapshot.fps,
                capture_fps = snapshot.capture_fps,
                received_fps = snapshot.received_fps,
                presented_fps = snapshot.presented_fps,
                rtt_ms = snapshot.rtt_ms,
                decode_ms = snapshot.decode_ms,
                present_ms = snapshot.present_ms,
                glass_p50_ms = ?snapshot.glass_to_glass_p50_ms,
                glass_p95_ms = ?snapshot.glass_to_glass_p95_ms,
                frames_sent = snapshot.frames_sent,
                frames_received = snapshot.frames_received,
                frames_presented = snapshot.frames_presented,
                dropped_frames = snapshot.dropped_frames,
                "媒体指标（30 秒）"
            );
        }
        if snapshot.fps > 0.0 {
            tracing::info!(
                fps = snapshot.fps,
                capture_fps = snapshot.capture_fps,
                capture_ms = snapshot.capture_ms,
                encode_ms = snapshot.encode_ms,
                bitrate_kbps = snapshot.bitrate_kbps,
                frames_sent = snapshot.frames_sent,
                rtt_ms = snapshot.rtt_ms,
                "媒体指标-发送（30 秒）"
            );
        }
    }
    ipc::emit(&state.app, ipc::EVENT_METRICS, snapshot);
}

async fn check_permissions(state: &Arc<AppState>) {
    let report = crate::permissions::report();
    let previous = state.permissions.read().clone();
    if previous == report {
        return;
    }
    let previous_missing = !previous.all_granted();
    let missing = !report.all_granted();
    *state.permissions.write() = report.clone();
    ipc::emit(&state.app, ipc::EVENT_PERMISSION_CHANGED, report);
    if previous_missing != missing {
        if missing {
            dispatch_session(state, SessionEvent::PermissionLost).await;
        } else {
            dispatch_session(state, SessionEvent::PermissionRestored).await;
        }
    }
}

fn emit_transfers(state: &AppState) {
    for view in state.transfers.lock().views() {
        ipc::emit(&state.app, ipc::EVENT_TRANSFER_PROGRESS, view);
    }
}

fn find_sending_transfer(
    state: &AppState,
    device: DeviceId,
    transfer_id: FileTransferId,
) -> Option<u64> {
    let transfers = state.transfers.lock();
    find_sending_in(&transfers.entries, device, transfer_id)
}

fn find_receiving_transfer(
    state: &AppState,
    device: DeviceId,
    transfer_id: FileTransferId,
) -> Option<u64> {
    let transfers = state.transfers.lock();
    find_receiving_in(&transfers.entries, device, transfer_id)
}

fn find_sending_in(
    entries: &std::collections::BTreeMap<u64, TransferEntry>,
    device: DeviceId,
    transfer_id: FileTransferId,
) -> Option<u64> {
    entries.iter().find_map(|(id, entry)| match entry {
        TransferEntry::Sending {
            device_id, view, ..
        } if *device_id == device && view.id == transfer_id.0 => Some(*id),
        _ => None,
    })
}

fn find_receiving_in(
    entries: &std::collections::BTreeMap<u64, TransferEntry>,
    device: DeviceId,
    transfer_id: FileTransferId,
) -> Option<u64> {
    entries.iter().find_map(|(id, entry)| match entry {
        TransferEntry::Receiving {
            device_id,
            transfer_id: protocol_id,
            ..
        } if *device_id == device && *protocol_id == transfer_id => Some(*id),
        _ => None,
    })
}

fn link_connected(state: &AppState, device: DeviceId) -> bool {
    state
        .links
        .lock()
        .get(&device)
        .is_some_and(|link| link.connected.load(Ordering::SeqCst))
}

fn notify_file_no_peer(state: &AppState) {
    let language = state.language();
    let (title, body) = i18n::file_no_peer(language);
    add_notification(
        state,
        Severity::Warning,
        NotificationCategory::FileTransfer,
        title,
        body,
        None,
    );
}

fn notify_file_drop_hint(state: &AppState) {
    let language = state.language();
    let enabled = state.settings.read().file_transfer_enabled;
    let (title, body) = if enabled {
        i18n::file_drop_hint(language)
    } else {
        i18n::file_disabled(language)
    };
    add_notification(
        state,
        if enabled {
            Severity::Info
        } else {
            Severity::Warning
        },
        NotificationCategory::FileTransfer,
        title,
        body,
        None,
    );
}

pub async fn send_files(
    state: &Arc<AppState>,
    paths: Vec<std::path::PathBuf>,
    target: Option<DeviceId>,
) {
    if !state.settings.read().file_transfer_enabled {
        let language = state.language();
        let (title, body) = i18n::file_disabled(language);
        add_notification(
            state,
            Severity::Warning,
            NotificationCategory::FileTransfer,
            title,
            body,
            None,
        );
        return;
    }
    if paths.is_empty() {
        return;
    }
    let Some(target) = target.or_else(|| state.session_peer_id()) else {
        notify_file_no_peer(state);
        return;
    };
    if !link_connected(state, target) {
        notify_file_no_peer(state);
        return;
    }
    for path in paths {
        let transfer_id = {
            let mut transfers = state.transfers.lock();
            FileTransferId(transfers.next_id())
        };
        let path_clone = path.clone();
        let sender = match tokio::task::spawn_blocking(move || {
            FileSender::new(transfer_id, &path_clone, FILE_CHUNK_SIZE)
        })
        .await
        {
            Ok(Ok(sender)) => sender,
            Ok(Err(error)) => {
                add_notification(
                    state,
                    Severity::Error,
                    NotificationCategory::FileTransfer,
                    i18n::file_failed(state.language(), &error.to_string()).0,
                    error.to_string(),
                    None,
                );
                continue;
            }
            Err(error) => {
                tracing::warn!(%error, "文件哈希任务失败");
                continue;
            }
        };
        let view = crate::ipc::TransferView {
            id: transfer_id.0,
            direction: crate::ipc::TransferDirection::Send,
            file_name: sender.file_name().to_string(),
            bytes_done: 0,
            bytes_total: sender.size(),
            state: crate::ipc::TransferState::Offered,
            message: None,
        };
        let offer = sender.offer();
        {
            let mut transfers = state.transfers.lock();
            transfers.entries.insert(
                transfer_id.0,
                TransferEntry::Sending {
                    device_id: target,
                    sender,
                    view: view.clone(),
                },
            );
        }
        ipc::emit(&state.app, ipc::EVENT_TRANSFER_PROGRESS, view);
        if !send_control_to(state, target, offer) {
            let reason = i18n::file_reason_no_peer(state.language());
            mark_transfer_failed(state, transfer_id.0, &reason);
        }
    }
}

fn fail_transfers_for_device(state: &AppState, device: DeviceId, reason: &str) {
    let mut transfers = state.transfers.lock();
    let mut changed = false;
    for entry in transfers.entries.values_mut() {
        let matches_device = match entry {
            TransferEntry::Sending { device_id, .. }
            | TransferEntry::Receiving { device_id, .. } => *device_id == device,
        };
        if !matches_device {
            continue;
        }
        if let TransferEntry::Receiving { receiver, .. } = entry {
            if let Some(receiver) = receiver.as_mut() {
                receiver.abort();
            }
        }
        let view = entry.view_mut();
        if matches!(
            view.state,
            crate::ipc::TransferState::Done
                | crate::ipc::TransferState::Failed
                | crate::ipc::TransferState::Cancelled
        ) {
            continue;
        }
        view.state = crate::ipc::TransferState::Failed;
        view.message = Some(reason.to_string());
        changed = true;
    }
    drop(transfers);
    if changed {
        emit_transfers(state);
        add_notification(
            state,
            Severity::Warning,
            NotificationCategory::FileTransfer,
            i18n::file_failed(state.language(), reason).0,
            i18n::file_failed(state.language(), reason).1,
            None,
        );
    }
}

fn mark_transfer_failed(state: &AppState, id: u64, message: &str) {
    if let Some(entry) = state.transfers.lock().entries.get_mut(&id) {
        entry.view_mut().state = crate::ipc::TransferState::Failed;
        entry.view_mut().message = Some(message.to_string());
    }
    emit_transfers(state);
    let language = state.language();
    let (title, body) = i18n::file_failed(language, message);
    add_notification(
        state,
        Severity::Error,
        NotificationCategory::FileTransfer,
        title,
        body,
        None,
    );
}

async fn advance_transfer(state: &Arc<AppState>, id: u64) {
    let chunk = {
        let mut transfers = state.transfers.lock();
        let Some(entry) = transfers.entries.get_mut(&id) else {
            return;
        };
        match entry {
            TransferEntry::Sending {
                device_id,
                sender,
                view,
            } => {
                if view.state != crate::ipc::TransferState::Transferring {
                    None
                } else {
                    match sender.next_chunk() {
                        Ok(Some(message)) => {
                            view.bytes_done = sender.progress().bytes_done;
                            Some(Ok((*device_id, message, view.clone(), true)))
                        }
                        Ok(None) => {
                            view.bytes_done = sender.size();
                            view.state = crate::ipc::TransferState::Verifying;
                            Some(Ok((*device_id, sender.complete(), view.clone(), false)))
                        }
                        Err(error) => {
                            view.state = crate::ipc::TransferState::Failed;
                            view.message = Some(error.to_string());
                            Some(Err(error.to_string()))
                        }
                    }
                }
            }
            TransferEntry::Receiving { .. } => None,
        }
    };
    match chunk {
        Some(Ok((device_id, message, view, more))) => {
            if !send_control_to(state, device_id, message) {
                let reason = i18n::file_reason_connection_lost(state.language());
                mark_transfer_failed(state, id, &reason);
                return;
            }
            ipc::emit(&state.app, ipc::EVENT_TRANSFER_PROGRESS, view);
            if more {
                let state_clone = Arc::clone(state);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    let _ = state_clone.events.send(AppEvent::TransferTick { id });
                });
            }
        }
        Some(Err(error)) => {
            add_notification(
                state,
                Severity::Error,
                NotificationCategory::FileTransfer,
                i18n::file_failed(state.language(), &error).0,
                error,
                None,
            );
            emit_transfers(state);
        }
        None => {}
    }
}

async fn handle_file_chunk(
    state: &Arc<AppState>,
    device: DeviceId,
    transfer_id: FileTransferId,
    offset: u64,
    data: Vec<u8>,
) {
    let Some(id) = find_receiving_transfer(state, device, transfer_id) else {
        return;
    };
    let mut transfers = state.transfers.lock();
    let Some(entry) = transfers.entries.get_mut(&id) else {
        return;
    };
    let TransferEntry::Receiving { receiver, view, .. } = entry else {
        return;
    };
    let Some(receiver) = receiver.as_mut() else {
        return;
    };
    if let Err(error) = receiver.write_chunk(offset, &data) {
        view.state = crate::ipc::TransferState::Failed;
        view.message = Some(error.to_string());
        let message = error.to_string();
        drop(transfers);
        send_control_to(
            state,
            device,
            ControlMessage::FileResult {
                transfer_id,
                success: false,
                message: message.clone(),
            },
        );
        add_notification(
            state,
            Severity::Error,
            NotificationCategory::FileTransfer,
            i18n::file_failed(state.language(), &message).0,
            message,
            None,
        );
        emit_transfers(state);
        return;
    }
    let progress = receiver.progress();
    view.bytes_done = progress.bytes_done;
    let view_clone = view.clone();
    drop(transfers);
    ipc::emit(&state.app, ipc::EVENT_TRANSFER_PROGRESS, view_clone);
}

async fn handle_file_complete(
    state: &Arc<AppState>,
    device: DeviceId,
    transfer_id: FileTransferId,
) {
    let Some(id) = find_receiving_transfer(state, device, transfer_id) else {
        return;
    };
    let (receiver, file_name) = {
        let mut transfers = state.transfers.lock();
        match transfers.entries.get_mut(&id) {
            Some(TransferEntry::Receiving { receiver, view, .. }) => {
                view.state = crate::ipc::TransferState::Verifying;
                (receiver.take(), view.file_name.clone())
            }
            _ => (None, String::new()),
        }
    };
    let Some(receiver) = receiver else {
        return;
    };
    let result = tokio::task::spawn_blocking(move || receiver.finish()).await;
    match result {
        Ok(Ok(_path)) => {
            if let Some(entry) = state.transfers.lock().entries.get_mut(&id) {
                entry.view_mut().state = crate::ipc::TransferState::Done;
                entry.view_mut().bytes_done = entry.view().bytes_total;
            }
            send_control_to(
                state,
                device,
                ControlMessage::FileResult {
                    transfer_id,
                    success: true,
                    message: "ok".into(),
                },
            );
            let (title, body) = i18n::file_received(state.language(), &file_name);
            add_notification(
                state,
                Severity::Success,
                NotificationCategory::FileTransfer,
                title,
                body,
                None,
            );
        }
        Ok(Err(error)) => {
            if let Some(entry) = state.transfers.lock().entries.get_mut(&id) {
                entry.view_mut().state = crate::ipc::TransferState::Failed;
                entry.view_mut().message = Some(error.to_string());
            }
            send_control_to(
                state,
                device,
                ControlMessage::FileResult {
                    transfer_id,
                    success: false,
                    message: error.to_string(),
                },
            );
            add_notification(
                state,
                Severity::Error,
                NotificationCategory::FileTransfer,
                i18n::file_failed(state.language(), &error.to_string()).0,
                error.to_string(),
                None,
            );
        }
        Err(error) => tracing::warn!(%error, "文件校验任务失败"),
    }
    emit_transfers(state);
}

pub fn clear_finished_transfers(state: &AppState) {
    state.transfers.lock().remove_finished();
    ipc::emit(
        &state.app,
        ipc::EVENT_TRANSFERS_CHANGED,
        state.transfers.lock().views(),
    );
}

pub async fn cancel_transfer(state: &Arc<AppState>, id: u64) {
    let entry = state.transfers.lock().entries.remove(&id);
    match entry {
        Some(TransferEntry::Sending {
            device_id,
            mut view,
            ..
        }) => {
            view.state = crate::ipc::TransferState::Cancelled;
            send_control_to(
                state,
                device_id,
                ControlMessage::FileResult {
                    transfer_id: FileTransferId(view.id),
                    success: false,
                    message: "cancelled".into(),
                },
            );
            ipc::emit(&state.app, ipc::EVENT_TRANSFER_PROGRESS, view);
        }
        Some(TransferEntry::Receiving {
            device_id,
            transfer_id,
            mut receiver,
            mut view,
        }) => {
            if let Some(receiver) = receiver.as_mut() {
                receiver.abort();
            }
            send_control_to(
                state,
                device_id,
                ControlMessage::FileResult {
                    transfer_id,
                    success: false,
                    message: "cancelled".into(),
                },
            );
            view.state = crate::ipc::TransferState::Cancelled;
            ipc::emit(&state.app, ipc::EVENT_TRANSFER_PROGRESS, view);
        }
        None => {}
    }
}

async fn handle_update_manifest(
    state: &Arc<AppState>,
    manifest: UpdateManifest,
    reply: Option<u64>,
) {
    let version = manifest.version.clone();
    let manager = {
        let update = state.update.lock();
        update.manager.clone()
    };
    {
        let update = state.update.lock();
        if update.incoming.is_some() || update.status.state == UpdateState::Applying {
            drop(update);
            send_control_on(
                state,
                reply,
                ControlMessage::UpdateDecision {
                    version,
                    accepted: false,
                },
            );
            return;
        }
    }
    match UpdateReceiver::new(&manager, manifest.clone()) {
        Ok(receiver) => {
            {
                let mut update = state.update.lock();
                update.incoming = Some(receiver);
                update.incoming_manifest = Some(manifest.clone());
                update.status.state = UpdateState::Receiving;
                update.status.version = Some(version.clone());
                update.status.kind = Some(manifest.kind);
                update.status.progress_percent = 0;
                update.status.message = None;
                let pushing = update.push.as_ref().is_some_and(|push| {
                    push.peers.iter().any(|peer| {
                        matches!(
                            peer.state,
                            UpdatePeerState::Queued
                                | UpdatePeerState::Connecting
                                | UpdatePeerState::Transferring
                        )
                    })
                });
                if !pushing {
                    update.status.peers.clear();
                }
            }
            send_control_on(
                state,
                reply,
                ControlMessage::UpdateDecision {
                    version: version.clone(),
                    accepted: true,
                },
            );
            emit_update_status(state);
        }
        Err(error) => {
            send_control_on(
                state,
                reply,
                ControlMessage::UpdateResult {
                    version,
                    success: false,
                    message: error.to_string(),
                },
            );
        }
    }
}

async fn handle_update_chunk(
    state: &Arc<AppState>,
    offset: u64,
    data: Vec<u8>,
    reply: Option<u64>,
) {
    let mut update = state.update.lock();
    let Some(receiver) = update.incoming.as_mut() else {
        return;
    };
    if let Err(error) = receiver.write_chunk(offset, &data) {
        let message = error.to_string();
        update.incoming = None;
        update.status.state = UpdateState::Failed;
        update.status.message = Some(message.clone());
        drop(update);
        send_control_on(
            state,
            reply,
            ControlMessage::UpdateResult {
                version: state
                    .update
                    .lock()
                    .incoming_manifest
                    .as_ref()
                    .map(|manifest| manifest.version.clone())
                    .unwrap_or_default(),
                success: false,
                message,
            },
        );
        emit_update_status(state);
        return;
    }
    let progress = receiver.progress();
    update.status.progress_percent = progress
        .bytes_done
        .saturating_mul(100)
        .checked_div(progress.bytes_total)
        .unwrap_or(0)
        .min(100) as u8;
    drop(update);
    emit_update_status(state);
}

async fn handle_update_complete(state: &Arc<AppState>, reply: Option<u64>) {
    let (receiver, manifest) = {
        let mut update = state.update.lock();
        let receiver = update.incoming.take();
        let manifest = update.incoming_manifest.clone();
        update.status.state = UpdateState::Staged;
        update.status.progress_percent = 100;
        (receiver, manifest)
    };
    let (Some(receiver), Some(manifest)) = (receiver, manifest) else {
        return;
    };
    let auto_accept = manifest.force || state.settings.read().update_auto_accept;
    let path = tokio::task::spawn_blocking(move || receiver.finish()).await;
    let path = match path {
        Ok(Ok(path)) => path,
        Ok(Err(error)) => {
            let message = error.to_string();
            state.update.lock().status.state = UpdateState::Failed;
            state.update.lock().status.message = Some(message.clone());
            emit_update_status(state);
            send_control_on(
                state,
                reply,
                ControlMessage::UpdateResult {
                    version: manifest.version.clone(),
                    success: false,
                    message,
                },
            );
            return;
        }
        Err(error) => {
            let message = error.to_string();
            state.update.lock().status.state = UpdateState::Failed;
            state.update.lock().status.message = Some(message.clone());
            emit_update_status(state);
            send_control_on(
                state,
                reply,
                ControlMessage::UpdateResult {
                    version: manifest.version.clone(),
                    success: false,
                    message,
                },
            );
            return;
        }
    };
    let manager = state.update.lock().manager.clone();
    {
        let mut update = state.update.lock();
        update.status.version = Some(manifest.version.clone());
        update.staged_path = Some(path.clone());
        update.refresh_rollback_versions();
    }
    if let Err(error) = manager.save_staged(&nc_update::StagedUpdate {
        manifest: manifest.clone(),
        path,
    }) {
        tracing::warn!(%error, "保存暂存更新记录失败");
    }
    emit_update_status(state);
    send_control_on(
        state,
        reply,
        ControlMessage::UpdateResult {
            version: manifest.version.clone(),
            success: true,
            message: "已校验并暂存".into(),
        },
    );
    let language = state.language();
    let (title, body) = i18n::update_staged(language, &manifest.version);
    add_notification(
        state,
        Severity::Success,
        NotificationCategory::Update,
        title,
        body,
        Some(NotificationAction {
            kind: NotificationActionKind::OpenUpdate,
            label: i18n::install_action(language),
            payload: Some(manifest.version.clone()),
        }),
    );
    if auto_accept {
        tokio::time::sleep(Duration::from_millis(150)).await;
        apply_staged_update(state, &manifest.version).await;
    }
}

pub async fn apply_staged_update(state: &Arc<AppState>, version: &str) {
    let (manager, path, manifest) = {
        let update = state.update.lock();
        let path = update.staged_path.clone();
        let manifest = update
            .incoming_manifest
            .clone()
            .filter(|manifest| manifest.version == version);
        (update.manager.clone(), path, manifest)
    };
    let (Some(path), Some(manifest)) = (path, manifest) else {
        return;
    };
    state.update.lock().status.state = UpdateState::Applying;
    emit_update_status(state);
    let result = tokio::task::spawn_blocking(move || manager.apply(&path, &manifest)).await;
    match result {
        Ok(Ok(outcome)) => {
            {
                let mut update = state.update.lock();
                update.status.state = UpdateState::Done;
                update.status.message = Some(outcome.message.clone());
                update.refresh_rollback_versions();
                update.manager.clear_staged();
            }
            emit_update_status(state);
            if outcome.restart_required {
                state.quitting.store(true, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(500)).await;
                state.app.exit(0);
            }
        }
        Ok(Err(error)) => {
            let language = state.language();
            let (title, body) = i18n::update_failed(language, &error.to_string());
            state.update.lock().status.state = UpdateState::Failed;
            state.update.lock().status.message = Some(error.to_string());
            emit_update_status(state);
            add_notification(
                state,
                Severity::Error,
                NotificationCategory::Update,
                title,
                body,
                None,
            );
        }
        Err(error) => tracing::warn!(%error, "应用更新任务失败"),
    }
}

pub async fn rollback_update(state: &Arc<AppState>, version: &str) {
    let manager = state.update.lock().manager.clone();
    let version_owned = version.to_string();
    let result = tokio::task::spawn_blocking(move || manager.rollback(&version_owned)).await;
    {
        state.update.lock().refresh_rollback_versions();
    }
    emit_update_status(state);
    match result {
        Ok(Ok(())) => {
            state.quitting.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(300)).await;
            state.app.exit(0);
        }
        Ok(Err(error)) => {
            add_notification(
                state,
                Severity::Error,
                NotificationCategory::Update,
                i18n::update_failed(state.language(), &error.to_string()).0,
                error.to_string(),
                None,
            );
        }
        Err(error) => tracing::warn!(%error, "回退任务失败"),
    }
}

pub fn package_kind_for_path(path: &std::path::Path) -> Option<PackageKind> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("msi") if Platform::current() == Platform::Windows => Some(PackageKind::Installer),
        Some("pkg") if Platform::current() == Platform::MacOs => Some(PackageKind::Installer),
        Some("exe") if Platform::current() == Platform::Windows => {
            if file_name.contains("setup") || file_name.contains("installer") {
                Some(PackageKind::Installer)
            } else {
                Some(PackageKind::Portable)
            }
        }
        _ => None,
    }
}

struct UpdateTarget {
    device_id: DeviceId,
    device_name: String,
    platform: Platform,
    address: Option<SocketAddr>,
}

fn update_targets(state: &AppState) -> Vec<UpdateTarget> {
    let trusted = state
        .trust
        .read()
        .map(|store| store.all())
        .unwrap_or_default();
    let devices = state.devices.read();
    trusted
        .into_iter()
        .map(|record| {
            let discovered = devices.get(&record.device_id);
            let address = discovered.and_then(|device| {
                (device.info.control_port != 0)
                    .then(|| SocketAddr::new(device.address.ip(), device.info.control_port))
            });
            UpdateTarget {
                device_id: record.device_id,
                device_name: record.name,
                platform: record.platform,
                address,
            }
        })
        .collect()
}

fn push_in_progress(state: &AppState) -> bool {
    state.update.lock().push.as_ref().is_some_and(|push| {
        push.peers.iter().any(|peer| {
            matches!(
                peer.state,
                UpdatePeerState::Queued
                    | UpdatePeerState::Connecting
                    | UpdatePeerState::Transferring
            )
        })
    })
}

pub async fn push_update(
    state: &Arc<AppState>,
    path: std::path::PathBuf,
    version: String,
    force: bool,
    kind: PackageKind,
) {
    if push_in_progress(state) {
        let language = state.language();
        add_notification(
            state,
            Severity::Warning,
            NotificationCategory::Update,
            i18n::update_push_busy(language).0,
            i18n::update_push_busy(language).1,
            None,
        );
        return;
    }
    let manager = state.update.lock().manager.clone();
    let path_clone = path.clone();
    let manifest = tokio::task::spawn_blocking(move || {
        manager.prepare_local_package(&path_clone, &version, kind)
    })
    .await;
    let mut manifest = match manifest {
        Ok(Ok(manifest)) => manifest,
        Ok(Err(error)) => {
            add_notification(
                state,
                Severity::Error,
                NotificationCategory::Update,
                i18n::update_failed(state.language(), &error.to_string()).0,
                error.to_string(),
                None,
            );
            return;
        }
        Err(error) => {
            tracing::warn!(%error, "生成更新清单失败");
            return;
        }
    };
    manifest.force = force;
    let targets = update_targets(state);
    if targets.is_empty() {
        let language = state.language();
        {
            let mut update = state.update.lock();
            update.status.state = UpdateState::Failed;
            update.status.message = Some(i18n::update_push_unavailable(language).1);
            update.status.peers.clear();
            update.push = None;
        }
        emit_update_status(state);
        let (title, body) = i18n::update_push_unavailable(language);
        add_notification(
            state,
            Severity::Warning,
            NotificationCategory::Update,
            title,
            body,
            None,
        );
        return;
    }

    let language = state.language();
    let peers: Vec<UpdatePeerPush> = targets
        .iter()
        .map(|target| {
            let runnable = target.address.is_some() && target.platform == Platform::current();
            let message = if target.platform != Platform::current() {
                Some(i18n::update_peer_platform_mismatch(language).to_string())
            } else if target.address.is_none() {
                Some(i18n::update_peer_offline(language).to_string())
            } else {
                None
            };
            UpdatePeerPush {
                device_id: target.device_id,
                device_name: target.device_name.clone(),
                state: if runnable {
                    UpdatePeerState::Queued
                } else {
                    UpdatePeerState::Failed
                },
                progress_percent: 0,
                message,
            }
        })
        .collect();
    {
        let mut update = state.update.lock();
        update.status.state = UpdateState::Receiving;
        update.status.version = Some(manifest.version.clone());
        update.status.kind = Some(manifest.kind);
        update.status.progress_percent = 0;
        update.status.message = None;
        update.status.peers = peers
            .iter()
            .map(|peer| UpdatePeerView {
                device_id: peer.device_id,
                device_name: peer.device_name.clone(),
                state: peer.state,
                progress_percent: peer.progress_percent,
                message: peer.message.clone(),
            })
            .collect();
        update.push = Some(UpdatePush {
            manifest: manifest.clone(),
            peers,
        });
    }
    emit_update_status(state);
    if !targets
        .iter()
        .any(|target| target.address.is_some() && target.platform == Platform::current())
    {
        let failed = targets.len();
        let mut update = state.update.lock();
        update.status.state = UpdateState::Failed;
        update.status.message = Some(i18n::update_push_finished(language, 0, failed));
        update.push = None;
        drop(update);
        emit_update_status(state);
        return;
    }
    for target in targets {
        let Some(address) = target
            .address
            .filter(|_| target.platform == Platform::current())
        else {
            continue;
        };
        let state_clone = Arc::clone(state);
        let path = path.clone();
        let manifest = manifest.clone();
        tokio::spawn(async move {
            let result =
                push_update_to_peer(&state_clone, target.device_id, address, &path, &manifest)
                    .await;
            match result {
                Ok(()) => set_update_peer_status(
                    &state_clone,
                    target.device_id,
                    UpdatePeerState::Done,
                    100,
                    None,
                ),
                Err(message) => set_update_peer_status(
                    &state_clone,
                    target.device_id,
                    UpdatePeerState::Failed,
                    0,
                    Some(message),
                ),
            }
        });
    }
}

fn set_update_peer_status(
    state: &AppState,
    device_id: DeviceId,
    peer_state: UpdatePeerState,
    progress_percent: u8,
    message: Option<String>,
) {
    let mut update = state.update.lock();
    let language = state.language();
    let already_final = matches!(
        update.status.state,
        UpdateState::Done | UpdateState::Failed
    );
    let mut finished = None;
    if let Some(push) = update.push.as_mut() {
        if let Some(peer) = push
            .peers
            .iter_mut()
            .find(|peer| peer.device_id == device_id)
        {
            peer.state = peer_state;
            peer.progress_percent = peer.progress_percent.max(progress_percent.min(100));
            if message.is_some() {
                peer.message = message;
            }
        }
        let peers: Vec<UpdatePeerView> = push
            .peers
            .iter()
            .map(|peer| UpdatePeerView {
                device_id: peer.device_id,
                device_name: peer.device_name.clone(),
                state: peer.state,
                progress_percent: peer.progress_percent,
                message: peer.message.clone(),
            })
            .collect();
        let total: u32 = push
            .peers
            .iter()
            .map(|peer| peer.progress_percent as u32)
            .sum();
        let progress = (total / push.peers.len().max(1) as u32) as u8;
        let active = push.peers.iter().any(|peer| {
            matches!(
                peer.state,
                UpdatePeerState::Queued
                    | UpdatePeerState::Connecting
                    | UpdatePeerState::Transferring
            )
        });
        let done = push
            .peers
            .iter()
            .filter(|peer| peer.state == UpdatePeerState::Done)
            .count();
        let failed = push.peers.len() - done;
        let final_progress = ((done.saturating_mul(100)) / push.peers.len().max(1)) as u8;
        update.status.peers = peers;
        update.status.progress_percent = if active { progress } else { final_progress };
        if !active {
            update.status.state = if done > 0 {
                UpdateState::Done
            } else {
                UpdateState::Failed
            };
            update.status.message = Some(i18n::update_push_finished(language, done, failed));
            if !already_final {
                finished = Some((done, failed));
            }
        }
    }
    drop(update);
    emit_update_status(state);
    if let Some((done, failed)) = finished {
        add_notification(
            state,
            if failed == 0 {
                Severity::Success
            } else if done == 0 {
                Severity::Error
            } else {
                Severity::Warning
            },
            NotificationCategory::Update,
            i18n::update_push_title(language),
            i18n::update_push_finished(language, done, failed),
            None,
        );
    }
}

fn push_text(state: &AppState, zh: &str, en: &str) -> String {
    match state.language() {
        nc_core::Language::Zh => zh.to_string(),
        nc_core::Language::En => en.to_string(),
    }
}

async fn push_update_to_peer(
    state: &Arc<AppState>,
    device_id: DeviceId,
    address: SocketAddr,
    path: &std::path::Path,
    manifest: &UpdateManifest,
) -> std::result::Result<(), String> {
    use tokio::io::AsyncReadExt;

    set_update_peer_status(state, device_id, UpdatePeerState::Connecting, 0, None);
    let config = control_config(state);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
    let connection = nc_control::ControlConnection::connect(
        config,
        address,
        PairingIntent::Trusted,
        ConnectionPurpose::Update,
        event_tx,
    )
    .await
    .map_err(|error| error.to_string())?;

    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match event_rx.recv().await {
                Some(ControlEvent::Connected { peer, .. }) => {
                    return if peer.id == device_id {
                        Ok(())
                    } else {
                        Err(push_text(
                            state,
                            "对端设备身份与目标不一致",
                            "Peer identity does not match the update target",
                        ))
                    };
                }
                Some(ControlEvent::Disconnected { reason })
                | Some(ControlEvent::PairingFailed { reason }) => return Err(reason),
                Some(_) => {}
                None => {
                    return Err(push_text(
                        state,
                        "更新连接已关闭",
                        "Update connection closed",
                    ))
                }
            }
        }
    })
    .await
    .map_err(|_| {
        push_text(
            state,
            "更新连接握手超时",
            "Update connection handshake timed out",
        )
    })??;

    connection
        .send(ControlMessage::UpdateManifest {
            manifest: manifest.clone(),
        })
        .map_err(|error| error.to_string())?;
    let accepted = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match event_rx.recv().await {
                Some(ControlEvent::Message(ControlMessage::UpdateDecision {
                    accepted, ..
                })) => return Ok(accepted),
                Some(ControlEvent::Message(ControlMessage::UpdateResult {
                    success,
                    message,
                    ..
                })) => {
                    return if success { Ok(true) } else { Err(message) };
                }
                Some(ControlEvent::Disconnected { reason })
                | Some(ControlEvent::PairingFailed { reason }) => return Err(reason),
                Some(_) => {}
                None => {
                    return Err(push_text(
                        state,
                        "更新连接已关闭",
                        "Update connection closed",
                    ))
                }
            }
        }
    })
    .await
    .map_err(|_| {
        push_text(
            state,
            "等待对端接受更新超时",
            "Timed out waiting for the peer to accept the update",
        )
    })??;
    if !accepted {
        return Err(push_text(
            state,
            "接收方拒绝本次更新",
            "The receiver rejected this update",
        ));
    }

    set_update_peer_status(state, device_id, UpdatePeerState::Transferring, 0, None);
    let mut file = tokio::fs::File::open(path).await.map_err(|error| {
        format!(
            "{}: {error}",
            push_text(state, "无法打开更新包", "Cannot open update package")
        )
    })?;
    let mut offset = 0u64;
    let mut last_emit = Instant::now();
    let mut buffer = vec![0u8; UPDATE_CHUNK_SIZE];
    loop {
        let read = file.read(&mut buffer).await.map_err(|error| {
            format!(
                "{}: {error}",
                push_text(state, "读取更新包失败", "Failed to read update package")
            )
        })?;
        if read == 0 {
            break;
        }
        connection
            .send(ControlMessage::UpdateChunk {
                version: manifest.version.clone(),
                offset,
                data: buffer[..read].to_vec(),
            })
            .map_err(|error| error.to_string())?;
        offset = offset.saturating_add(read as u64);
        if last_emit.elapsed() >= Duration::from_millis(80) {
            last_emit = Instant::now();
            let progress = offset
                .saturating_mul(100)
                .checked_div(manifest.size)
                .unwrap_or(0)
                .min(100) as u8;
            set_update_peer_status(
                state,
                device_id,
                UpdatePeerState::Transferring,
                progress,
                None,
            );
        }
        while let Ok(event) = event_rx.try_recv() {
            if let ControlEvent::Disconnected { reason } | ControlEvent::PairingFailed { reason } =
                event
            {
                return Err(reason);
            }
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    connection
        .send(ControlMessage::UpdateComplete {
            version: manifest.version.clone(),
        })
        .map_err(|error| error.to_string())?;
    set_update_peer_status(state, device_id, UpdatePeerState::Transferring, 100, None);

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match event_rx.recv().await {
                Some(ControlEvent::Message(ControlMessage::UpdateResult {
                    success,
                    message,
                    ..
                })) => {
                    return if success { Ok(()) } else { Err(message) };
                }
                Some(ControlEvent::Disconnected { reason })
                | Some(ControlEvent::PairingFailed { reason }) => return Err(reason),
                Some(_) => {}
                None => {
                    return Err(push_text(
                        state,
                        "更新连接已关闭",
                        "Update connection closed",
                    ))
                }
            }
        }
    })
    .await
    .map_err(|_| {
        push_text(
            state,
            "等待对端确认更新超时",
            "Timed out waiting for the peer to confirm the update",
        )
    })??;
    drop(connection);
    Ok(())
}

fn emit_update_status(state: &AppState) {
    let status = state.update.lock().status.clone();
    ipc::emit(&state.app, ipc::EVENT_UPDATE_STATUS, status);
}

pub fn build_device_info(
    identity: &nc_crypto::Identity,
    device_name: &str,
    control_port: u16,
    media_port: u16,
    video: &crate::runtime::VideoCapabilities,
) -> DeviceInfo {
    let (max_width, max_height, max_fps) = video.limits();
    let mut capabilities = nc_core::Capabilities::current();
    capabilities.hardware_encode = video.hardware_encode;
    capabilities.max_width = max_width;
    capabilities.max_height = max_height;
    capabilities.max_fps = max_fps;
    DeviceInfo {
        id: identity.device_id(),
        name: device_name.to_string(),
        platform: nc_core::Platform::current(),
        capabilities,
        control_port,
        media_port,
    }
}

pub fn build_session(settings: &AppSettings) -> nc_session::SessionMachine {
    nc_session::SessionMachine::from_settings(settings)
}

pub fn emit_notifications(state: &AppState) {
    ipc::emit(
        &state.app,
        ipc::EVENT_NOTIFICATIONS_CHANGED,
        state.notifications.lock().list(),
    );
}

pub async fn sync_ui_state(state: &Arc<AppState>, ui_state: UiState) {
    dispatch_session(state, SessionEvent::UiStateChanged { ui_state }).await;
}

pub fn settings_snapshot(state: &AppState) -> AppSettings {
    state.settings.read().clone()
}

pub fn layout_snapshot(state: &AppState) -> Layout {
    state.layout.read().clone()
}

pub fn update_layout(state: &Arc<AppState>, mut layout: Layout) {
    if let Err(error) = layout.validate() {
        tracing::warn!(%error, "布局校验失败");
        return;
    }
    let previous_overrides = state.layout.read().orientation_overrides.clone();
    if layout.apply_orientation_overrides() {
        layout.revision = layout.revision.saturating_add(1);
    }
    let orientation_changed = previous_overrides != layout.orientation_overrides;
    *state.layout.write() = layout.clone();
    save_layout(state, &layout);
    ipc::emit(&state.app, ipc::EVENT_LAYOUT_CHANGED, layout.clone());
    if orientation_changed {
        broadcast_topology(state);
    }
    let state_clone = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        dispatch_session(&state_clone, SessionEvent::LayoutUpdated { layout }).await;
        ensure_remote_stage_input(&state_clone).await;
    });
}

pub fn ensure_discovery(state: &Arc<AppState>) {
    start_discovery(state);
}

pub fn app_handle(state: &AppState) -> &AppHandle {
    &state.app
}

pub fn snapshot(state: &AppState) -> SessionSnapshot {
    state.snapshot()
}

pub fn active_connection(state: &AppState) -> Option<Arc<nc_control::ControlConnection>> {
    state.session_peer_id().and_then(|device| {
        state
            .links
            .lock()
            .get(&device)
            .map(|link| Arc::clone(&link.connection))
    })
}

pub fn pairing_snapshot(state: &AppState) -> Option<PairingState> {
    state.pairing.lock().clone()
}

pub fn post_event(state: &AppState, event: AppEvent) -> bool {
    state.events.send(event).is_ok()
}

pub async fn post_session(state: &Arc<AppState>, event: SessionEvent) {
    dispatch_session(state, event).await;
}

pub fn stop_discovery(state: &AppState) {
    if let Some(service) = state.discovery.lock().take() {
        service.stop();
    }
}

pub async fn set_ui_state(state: &Arc<AppState>, ui_state: UiState) {
    dispatch_session(state, SessionEvent::UiStateChanged { ui_state }).await;
    match ui_state {
        UiState::Background => {
            let _ = window::hide_to_background(&state.app);
        }
        UiState::ControlPanel => {
            let _ = window::show_control_panel(&state.app);
        }
        UiState::Stage => {
            schedule_stage_show(state);
        }
    }
}

pub fn show_control_panel(state: &AppState) {
    let _ = window::show_control_panel(&state.app);
}

pub fn request_quit(state: &AppState) {
    state.quitting.store(true, Ordering::SeqCst);
    state.app.exit(0);
}

pub fn report_webcodecs(state: &AppState, probe: ipc::WebCodecsProbe) {
    let _ = state.events.send(AppEvent::WebCodecs(probe));
}

#[allow(clippy::too_many_arguments)]
pub fn report_present_stats(
    state: &AppState,
    decode_ms: f64,
    present_ms: f64,
    frames: u32,
    presented_frames: u32,
    dropped_frames: u32,
    timestamp_us: u64,
) {
    let _ = state.events.send(AppEvent::PresentStats {
        decode_ms,
        present_ms,
        frames,
        presented_frames,
        dropped_frames,
        timestamp_us,
    });
}

const MAX_GLASS_ESTIMATE_MS: u64 = 10_000;

fn estimate_glass_to_glass_ms(
    now_ms: u64,
    frame_timestamp_us: u64,
    clock_offset_us: i64,
) -> Option<f64> {
    if frame_timestamp_us == 0 {
        return None;
    }
    let corrected_us = frame_timestamp_us as i64 + clock_offset_us;
    if corrected_us <= 0 {
        return None;
    }
    let frame_ms = corrected_us as f64 / 1000.0;
    let now_ms = now_ms as f64;
    if now_ms < frame_ms {
        return None;
    }
    let elapsed = now_ms - frame_ms;
    (elapsed <= MAX_GLASS_ESTIMATE_MS as f64).then_some(elapsed)
}

pub fn request_keyframe(state: &AppState, stream_id: StreamId) {
    state.media.request_keyframe(stream_id);
    send_control(state, ControlMessage::KeyframeRequest { stream_id });
}

pub fn begin_pairing(state: &Arc<AppState>, device: DeviceId) {
    let snapshot = state.snapshot();
    let role = *state.session_role.read();
    let released = state.input.lock().remote_input_released;
    if snapshot.state.is_active()
        || state.session_peer_id().is_some()
        || state.pending_connect.read().is_some()
        || state.pending_session.read().is_some()
        || (role == Some(SessionRole::Target) && !released)
    {
        let language = state.language();
        let (title, body) = i18n::pair_while_controlling(language);
        add_notification(
            state,
            Severity::Warning,
            NotificationCategory::Pairing,
            title,
            body,
            None,
        );
        return;
    }
    *state.pending_connect.write() = Some(device);
    set_pending_session(state, Some(device));
    state.user_disconnect.store(false, Ordering::SeqCst);
    let state_clone = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        dispatch_session(
            &state_clone,
            SessionEvent::ConnectRequested {
                device,
                trusted: false,
            },
        )
        .await;
    });
}

pub async fn remove_trusted_device(
    state: &Arc<AppState>,
    device_id: DeviceId,
) -> nc_core::Result<()> {
    {
        let mut store = state
            .trust
            .write()
            .map_err(|_| nc_core::NexusError::Internal("信任存储锁已损坏".into()))?;
        if store.get(&device_id).is_none() {
            return Err(nc_core::NexusError::Business("设备不在信任列表中".into()));
        }
        let mut next = store.clone();
        next.remove(&device_id);
        next.save(&state.paths.trust_file)
            .map_err(|error| nc_core::NexusError::Crypto(error.to_string()))?;
        *store = next;
    }

    let active_device = state.session_peer_id();
    let pending_device = *state.pending_connect.read();
    let pending_session = *state.pending_session.read();
    let pairing_snapshot = state.pairing.lock().clone();
    let pairing_device = pairing_snapshot
        .as_ref()
        .and_then(|pairing| pairing.device_id);
    if active_device == Some(device_id) {
        let was_host = *state.session_role.read() == Some(SessionRole::Host);
        if state.input.lock().controlling_remote {
            send_control(state, ControlMessage::ReleaseAll);
        }
        if was_host {
            broadcast_host_status(state, false);
        }
        *state.session_peer.write() = None;
        *state.session_role.write() = None;
        *state.session_peer_mode.write() = None;
        state.media.clear_session();
        state.media.stop_all_outgoing().await;
        reset_input_runtime(state);
        dispatch_session(state, SessionEvent::RemoteSessionClosed).await;
    }
    if *state.group_host.read() == Some(device_id) {
        set_group_host(state, None);
    }
    if pending_device == Some(device_id) {
        *state.pending_connect.write() = None;
    }
    if pending_session == Some(device_id) {
        set_pending_session(state, None);
        set_switching_target(state, false);
    }
    let takeover_for_device = state
        .pending_takeover
        .lock()
        .as_ref()
        .is_some_and(|request| request.host == device_id || request.target == device_id);
    if takeover_for_device {
        clear_pending_takeover(state);
    }
    if pairing_device == Some(device_id) {
        state.pairing.lock().take();
    }
    remove_link(state, device_id);
    let file_reason = i18n::file_reason_connection_lost(state.language());
    fail_transfers_for_device(state, device_id, &file_reason);
    state.shared_sync.lock().acks.remove(&device_id);
    emit_shared_sync_status(state);
    if pairing_device == Some(device_id) {
        ipc::emit(
            &state.app,
            ipc::EVENT_PAIRING,
            PairingEventPayload {
                stage: PairingStage::Cancelled,
                device_id: Some(device_id),
                device_name: pairing_snapshot.and_then(|pairing| pairing.device_name),
                sas: None,
                message: None,
            },
        );
    }

    let mut layout = state.layout.read().clone();
    if drop_device_nodes(&mut layout, device_id) {
        layout.revision = layout.revision.saturating_add(1);
    }
    update_layout(state, layout);
    emit_devices(state);
    Ok(())
}

fn drop_device_nodes(layout: &mut Layout, device_id: DeviceId) -> bool {
    let before = layout.nodes.len();
    layout.nodes.retain(|node| node.device_id != device_id);
    if layout.nodes.len() == before {
        return false;
    }
    let remaining: std::collections::HashSet<DisplayNodeId> =
        layout.nodes.iter().map(|node| node.id.clone()).collect();
    layout.active_node_ids.retain(|id| remaining.contains(id));
    layout
        .orientation_overrides
        .retain(|id, _| remaining.contains(id));
    layout
        .remote_desktop_origins
        .retain(|id, _| remaining.contains(id));
    true
}

pub async fn retry_connection(state: &Arc<AppState>) {
    state.user_disconnect.store(false, Ordering::SeqCst);
    state.reconnect_attempt.store(0, Ordering::SeqCst);
    if let Some(handle) = state.reconnect_task.lock().take() {
        handle.abort();
    }
    let pending_target = *state.pending_session.read();
    if let Some(pending) = pending_target {
        restart_pending_session_timer(state);
        let linked = state.links.lock().contains_key(&pending);
        if linked {
            tracing::info!(%pending, "手动重连：清理旧会话并重发会话打开");
            send_control_to(state, pending, ControlMessage::SessionClose);
            send_session_open(state, pending);
        } else {
            tracing::info!(%pending, "手动重连：重新建立组连接");
            ensure_group_link(state, pending, PairingIntent::Trusted, true).await;
        }
        return;
    }
    let takeover = state
        .pending_takeover
        .lock()
        .as_ref()
        .map(|request| (request.host, request.target));
    if let Some((host, target)) = takeover {
        tracing::info!(%host, %target, "手动重连：重发主机释放请求");
        let sent = send_control_to(
            state,
            host,
            ControlMessage::HostReleaseRequest {
                requester: state.identity.device_id(),
            },
        );
        if !sent {
            clear_pending_takeover(state);
            let language = state.language();
            let host_name = device_display_name(state, host);
            let (title, body) = i18n::host_takeover_timed_out(language, &host_name);
            add_notification(
                state,
                Severity::Warning,
                NotificationCategory::Connection,
                title,
                body,
                Some(NotificationAction {
                    kind: NotificationActionKind::RetryConnection,
                    label: i18n::retry_action(language),
                    payload: Some(target.to_hex()),
                }),
            );
            return;
        }
        state.pending_takeover.lock().as_mut().unwrap().started = Instant::now();
        return;
    }
    let Some(peer) = state.session_peer_id() else {
        return;
    };
    if *state.session_role.read() != Some(SessionRole::Host) {
        return;
    }
    if !state.snapshot().stage_visible {
        return;
    }
    tracing::info!(%peer, "手动重连：重启媒体流");
    stop_media(state).await;
    start_media(state, peer).await;
    state.media.request_all_keyframes();
    send_control(
        state,
        ControlMessage::KeyframeRequest {
            stream_id: StreamId::VIDEO,
        },
    );
}

pub async fn begin_connect(state: &Arc<AppState>, device: DeviceId) -> nc_core::Result<()> {
    let local = state.identity.device_id();
    if device == local {
        return Err(nc_core::NexusError::InvalidInput("不能控制本机".into()));
    }
    if !target_is_available(state, device) {
        let language = state.language();
        return Err(nc_core::NexusError::Business(
            i18n::device_offline(language).0,
        ));
    }
    state.user_disconnect.store(false, Ordering::SeqCst);
    let role = *state.session_role.read();
    let released = state.input.lock().remote_input_released;
    clear_released_host_lock(state, role, released);

    let pending_target = *state.pending_session.read();
    match pending_request_action(pending_target, device) {
        PendingRequestAction::Proceed => {}
        PendingRequestAction::Resend => {
            restart_pending_session_timer(state);
            retry_pending_session(state, device).await;
            return Ok(());
        }
        PendingRequestAction::Override => {
            abort_pending_session(state, "用户选择新的控制目标", false).await;
        }
    }

    let current_host = *state.group_host.read();
    if let Some(host) = current_host {
        if host != local {
            let linked = state
                .links
                .lock()
                .get(&host)
                .is_some_and(|link| link.connected.load(Ordering::SeqCst));
            if linked {
                request_host_release(state, host, device)?;
                return Ok(());
            }
            tracing::warn!(%host, "主机不在直连中，清除陈旧主机锁");
            set_group_host(state, None);
        }
    }

    let role = *state.session_role.read();
    let released = state.input.lock().remote_input_released;
    if !manual_control_allowed(role, released) {
        let language = state.language();
        let (_, body) = i18n::controlled_by_other(language);
        return Err(nc_core::NexusError::Business(body));
    }
    if state.session_peer_id() == Some(device) && role == Some(SessionRole::Host) {
        return Err(nc_core::NexusError::Business(
            "已连接该设备，请从屏幕边缘或全屏视图继续控制".into(),
        ));
    }

    set_pending_session(state, Some(device));
    if state.snapshot().state.is_active() {
        switch_session_target(state, device).await;
        return Ok(());
    }

    *state.pending_connect.write() = Some(device);
    dispatch_session(
        state,
        SessionEvent::ConnectRequested {
            device,
            trusted: true,
        },
    )
    .await;
    Ok(())
}

fn request_host_release(
    state: &Arc<AppState>,
    host: DeviceId,
    target: DeviceId,
) -> nc_core::Result<()> {
    let local = state.identity.device_id();
    let sent = send_control_to(
        state,
        host,
        ControlMessage::HostReleaseRequest { requester: local },
    );
    if !sent {
        return Err(nc_core::NexusError::Business(
            "无法联系当前主机，请稍后重试".into(),
        ));
    }
    let previous = state.pending_takeover.lock().replace(PendingTakeover {
        host,
        target,
        started: Instant::now(),
    });
    if previous
        .as_ref()
        .map(|request| (request.host, request.target))
        != Some((host, target))
    {
        tracing::info!(%host, %target, "请求主机释放");
        emit_group_status(state);
    }
    Ok(())
}

async fn retry_pending_session(state: &Arc<AppState>, target: DeviceId) {
    let linked = state
        .links
        .lock()
        .get(&target)
        .is_some_and(|link| link.connected.load(Ordering::SeqCst));
    if linked {
        send_session_open(state, target);
    } else {
        ensure_group_link(state, target, PairingIntent::Trusted, true).await;
    }
}

async fn switch_session_target(state: &Arc<AppState>, target: DeviceId) {
    let previous = state.session_peer_id();
    let role = *state.session_role.read();
    let released = state.input.lock().remote_input_released;
    if !should_switch_session(previous, target, role, released) {
        if *state.pending_session.read() == Some(target) {
            set_pending_session(state, None);
        }
        return;
    }
    tracing::info!(?previous, %target, ?role, released, "切换控制目标");
    if state.input.lock().controlling_remote {
        send_control(state, ControlMessage::ReleaseAll);
    }
    let keep_stage_input = state.snapshot().stage_visible
        && state.snapshot().mode == Mode::Aggregation;
    *state.session_role.write() = Some(SessionRole::Host);
    if let Some(previous) = previous {
        send_control_to(state, previous, ControlMessage::SessionClose);
    }
    state.media.clear_session();
    state.media.stop_all_outgoing().await;
    if keep_stage_input {
        set_switching_target(state, true);
        reset_input_runtime(state);
        state.input.lock().set_suppressed(true);
    } else {
        reset_input_runtime(state);
    }
    *state.session_peer.write() = None;
    *state.session_peer_mode.write() = None;
    clear_pending_takeover(state);
    set_pending_session(state, Some(target));
    if !keep_stage_input {
        dispatch_session(state, SessionEvent::ReleaseAll).await;
    }

    ensure_group_link(state, target, PairingIntent::Trusted, true).await;
}

pub async fn update_settings(state: &Arc<AppState>, settings: AppSettings) {
    if let Err(error) = settings.validate() {
        tracing::warn!(%error, "设置校验失败");
        return;
    }
    let previous = state.settings.read().clone();
    let incoming_shared_revision = settings.shared_revision;
    let mut next = settings;
    next.shared_revision = previous.shared_revision;
    next.shared_updated_at_ms = previous.shared_updated_at_ms;
    next.shared_source = previous.shared_source;
    if incoming_shared_revision < previous.shared_revision {
        next.apply_shared_config(&previous.shared_config());
    }
    let shared_changed = next.shared_config() != previous.shared_config();
    let shared_snapshot = if shared_changed {
        Some(next.bump_shared_config(state.identity.device_id(), crate::notifications::now_ms()))
    } else {
        None
    };

    *state.settings.write() = next.clone();
    if let Err(error) = config::save_settings(&state.paths, &next) {
        tracing::warn!(%error, "保存设置失败");
    }
    if previous.mode != next.mode {
        dispatch_session(state, SessionEvent::SetMode { mode: next.mode }).await;
    }
    apply_settings_side_effects(state, &previous, &next).await;
    if let Some(snapshot) = shared_snapshot {
        send_shared_config(state, &snapshot);
        emit_shared_sync_status(state);
    }
    emit_settings_changed(state, &next);
    update_tray(state, &state.snapshot());
}

async fn apply_settings_side_effects(
    state: &Arc<AppState>,
    previous: &AppSettings,
    settings: &AppSettings,
) {
    if previous.language != settings.language || previous.device_name != settings.device_name {
        let monitors = nc_platform::monitors().unwrap_or_default();
        *state.monitors.write() = monitors;
        reconcile_local_layout(state);
    }
    {
        let mut device_info = state.device_info.write();
        device_info.name = settings.device_name.clone();
    }
    if previous.discovery_port != settings.discovery_port
        || previous.device_name != settings.device_name
    {
        if let Some(service) = state.discovery.lock().take() {
            service.stop();
        }
        start_discovery(state);
    }

    let peer = state.peer();
    let media_changed = previous.audio_enabled != settings.audio_enabled
        || peer.as_ref().is_some_and(|peer| {
            previous.video_target_for(peer.id) != settings.video_target_for(peer.id)
        });
    if !media_changed {
        return;
    }
    {
        let mut adaptive = state.adaptive.lock();
        adaptive.level = 0;
        adaptive.bad_ticks = 0;
        adaptive.good_ticks = 0;
    }
    let snapshot = state.snapshot();
    if snapshot.state != nc_core::SessionState::Aggregating || !snapshot.stage_visible {
        return;
    }
    let Some(peer) = peer else {
        return;
    };
    let target = target_for_peer(settings.video_target_for(peer.id), &peer);
    *state.first_frame_wait.lock() = Some((StreamId::VIDEO, Instant::now()));
    send_control(
        state,
        ControlMessage::StreamRequest {
            request: StreamRequest {
                stream_id: StreamId::VIDEO,
                target,
                audio: settings.audio_enabled,
                monitor_index: current_monitor_index(state),
            },
        },
    );
}

fn send_shared_config_to(
    state: &AppState,
    device: DeviceId,
    snapshot: &nc_core::SharedConfigSnapshot,
) -> bool {
    send_control_to(
        state,
        device,
        ControlMessage::SharedConfig {
            snapshot: snapshot.clone(),
        },
    )
}

fn send_shared_config(state: &AppState, snapshot: &nc_core::SharedConfigSnapshot) {
    let devices: Vec<DeviceId> = state.links.lock().keys().copied().collect();
    for device in devices {
        send_shared_config_to(state, device, snapshot);
    }
}

fn emit_settings_changed(state: &AppState, settings: &AppSettings) {
    ipc::emit(&state.app, ipc::EVENT_SETTINGS_CHANGED, settings.clone());
}

pub fn shared_config_status(state: &AppState) -> ipc::SharedConfigStatusView {
    let revision = state.settings.read().shared_revision;
    let trusted: std::collections::HashSet<DeviceId> = state
        .trust
        .read()
        .map(|store| {
            store
                .all()
                .into_iter()
                .map(|entry| entry.device_id)
                .collect()
        })
        .unwrap_or_default();
    let online: Vec<DeviceId> = state
        .links
        .lock()
        .iter()
        .filter(|(device, link)| trusted.contains(device) && link.connected.load(Ordering::SeqCst))
        .map(|(device, _)| *device)
        .collect();
    let acks = state.shared_sync.lock();
    compute_shared_config_status(revision, &online, &acks.acks, trusted.len())
}

fn compute_shared_config_status(
    revision: u64,
    online: &[DeviceId],
    acks: &std::collections::HashMap<DeviceId, u64>,
    total_devices: usize,
) -> ipc::SharedConfigStatusView {
    let synced_devices = online
        .iter()
        .filter(|device| acks.get(*device).is_some_and(|ack| *ack >= revision))
        .count();
    let online_devices = online.len();
    let state = if total_devices == 0 || online_devices == 0 {
        "local"
    } else if synced_devices == online_devices {
        "synced"
    } else {
        "syncing"
    };
    ipc::SharedConfigStatusView {
        state,
        revision,
        synced_devices,
        online_devices,
        total_devices,
    }
}

fn emit_shared_sync_status(state: &AppState) {
    ipc::emit(
        &state.app,
        ipc::EVENT_SHARED_SYNC_STATUS,
        shared_config_status(state),
    );
}

pub fn force_shared_config_sync(state: &AppState) {
    let snapshot = state
        .settings
        .read()
        .shared_snapshot(state.identity.device_id());
    send_shared_config(state, &snapshot);
    emit_shared_sync_status(state);
}

async fn handle_shared_config(
    state: &Arc<AppState>,
    snapshot: nc_core::SharedConfigSnapshot,
    from: DeviceId,
) {
    if let Err(error) = snapshot.config.validate() {
        tracing::warn!(%error, "拒绝非法的远端共享配置");
        return;
    }
    apply_shared_config(state, snapshot).await;
    let revision = state.settings.read().shared_revision;
    send_control_to(state, from, ControlMessage::SharedConfigAck { revision });
    emit_shared_sync_status(state);
}

async fn apply_shared_config(state: &Arc<AppState>, snapshot: nc_core::SharedConfigSnapshot) {
    let previous = state.settings.read().clone();
    let local_unversioned = previous.shared_revision == 0
        && previous.shared_updated_at_ms == 0
        && previous.shared_source.is_none();
    let incoming_unversioned = snapshot.revision == 0 && snapshot.updated_at_ms == 0;
    if local_unversioned && incoming_unversioned && snapshot.source < state.identity.device_id() {
        let ours = previous.shared_snapshot(state.identity.device_id());
        if ours.config != snapshot.config {
            send_shared_config(state, &ours);
        }
        return;
    }
    let mut next = previous.clone();
    if next.apply_shared_snapshot(&snapshot) {
        *state.settings.write() = next.clone();
        if let Err(error) = config::save_settings(&state.paths, &next) {
            tracing::warn!(%error, "保存共享配置失败");
        }
        tracing::info!(
            revision = snapshot.revision,
            source = %snapshot.source,
            "应用远端共享配置"
        );
        apply_settings_side_effects(state, &previous, &next).await;
        emit_settings_changed(state, &next);
        update_tray(state, &state.snapshot());
        send_shared_config(state, &next.shared_snapshot(snapshot.source));
        return;
    }
    let ours = next.shared_snapshot(state.identity.device_id());
    if ours.is_newer_than(
        snapshot.revision,
        snapshot.updated_at_ms,
        Some(snapshot.source),
    ) {
        send_shared_config(state, &ours);
    }
}

pub fn set_layout(state: &Arc<AppState>, layout: Layout) {
    update_layout(state, layout);
}

pub fn mark_notification_read(state: &AppState, id: u64) {
    let changed = state.notifications.lock().mark_read(id);
    if changed {
        emit_notifications(state);
        update_tray(state, &state.snapshot());
    }
}

pub fn mark_all_notifications_read(state: &AppState) {
    state.notifications.lock().mark_all_read();
    emit_notifications(state);
    update_tray(state, &state.snapshot());
}

pub fn clear_notifications(state: &AppState) {
    state.notifications.lock().clear();
    emit_notifications(state);
    update_tray(state, &state.snapshot());
}

pub async fn request_permission(state: &Arc<AppState>, kind: nc_core::PermissionKind) -> bool {
    let granted = crate::permissions::request(kind).unwrap_or(false);
    let report = crate::permissions::report();
    *state.permissions.write() = report.clone();
    ipc::emit(&state.app, ipc::EVENT_PERMISSION_CHANGED, report);
    granted
}

pub fn devices_snapshot(state: &AppState) -> Vec<ipc::DeviceView> {
    state.device_views()
}

#[cfg(windows)]
pub fn notify_input_service_update(state: &Arc<AppState>) {
    let status = crate::input_service::status();
    if !status.installed || !status.needs_update {
        return;
    }
    let (title, body) = i18n::input_service_needs_update(state.language());
    add_notification(
        state,
        Severity::Warning,
        NotificationCategory::System,
        title,
        body,
        None,
    );
}

#[cfg(not(windows))]
pub fn notify_input_service_update(_state: &Arc<AppState>) {}

pub fn notify_shortcut_failure(state: &Arc<AppState>, detail: &str) {
    let language = state.language();
    let (title, body) = i18n::shortcut_registration_failed(language, detail);
    add_notification(
        state,
        Severity::Warning,
        NotificationCategory::System,
        title,
        body,
        Some(NotificationAction {
            kind: NotificationActionKind::OpenSettings,
            label: i18n::open_settings_action(language),
            payload: None,
        }),
    );
}

pub fn notifications_snapshot(state: &AppState) -> Vec<AppNotification> {
    state.notifications.lock().list()
}

pub fn metrics_snapshot(state: &AppState) -> nc_observability::MetricsSnapshot {
    state.metrics.snapshot()
}

pub fn update_status_snapshot(state: &AppState) -> ipc::UpdateStatusView {
    state.update.lock().status.clone()
}

pub fn transfers_snapshot(state: &AppState) -> Vec<ipc::TransferView> {
    state.transfers.lock().views()
}

pub fn permissions_snapshot(state: &AppState) -> nc_core::PermissionReport {
    state.permissions.read().clone()
}

pub fn media_bridge_info(state: &AppState) -> ipc::MediaBridgeInfo {
    state.bridge_info.clone()
}

pub fn webcodecs_snapshot(state: &AppState) -> ipc::WebCodecsProbe {
    state.webcodecs.read().clone()
}

pub fn refresh_tray(state: &AppState) {
    update_tray(state, &state.snapshot());
}

pub fn set_tray(state: &AppState, tray: crate::tray::TrayHandle) {
    *state.tray.write() = Some(tray);
    update_tray(state, &state.snapshot());
}

pub fn default_layout(state: &AppState) -> Layout {
    Layout::new(local_display_nodes(state))
}

pub fn confirm_pairing(state: &AppState, accept: bool) -> nc_core::Result<()> {
    let device = state
        .pairing
        .lock()
        .as_ref()
        .and_then(|pairing| pairing.device_id);
    let guard = state.links.lock();
    let Some(link) = device.and_then(|device| guard.get(&device)) else {
        return Err(nc_core::NexusError::NotConnected("没有配对连接".into()));
    };
    link.connection.confirm_sas(accept)
}

pub async fn cancel_pairing(state: &Arc<AppState>) {
    let device = state
        .pairing
        .lock()
        .as_ref()
        .and_then(|pairing| pairing.device_id);
    if let Some(device) = device {
        remove_link(state, device);
    }
    state.pairing.lock().take();
    dispatch_session(
        state,
        SessionEvent::PairingFailed {
            reason: "用户取消".into(),
        },
    )
    .await;
    ipc::emit(
        &state.app,
        ipc::EVENT_PAIRING,
        PairingEventPayload {
            stage: PairingStage::Cancelled,
            device_id: None,
            device_name: None,
            sas: None,
            message: Some("用户取消".into()),
        },
    );
}

pub async fn summon_stage(state: &Arc<AppState>) {
    let snapshot = state.snapshot();
    if snapshot.state == nc_core::SessionState::Kvm {
        dispatch_session(
            state,
            SessionEvent::SetMode {
                mode: Mode::Aggregation,
            },
        )
        .await;
    }
    dispatch_session(state, SessionEvent::StageSummon).await;
}

pub async fn hide_stage(state: &Arc<AppState>) {
    let was_visible = state.snapshot().stage_visible;
    dispatch_session(state, SessionEvent::StageHide).await;
    if was_visible {
        block_nearest_local_edge(state);
        show_edge_release_osd(state, None, false);
    }
}

fn block_nearest_local_edge(state: &AppState) {
    let Ok((x, y)) = nc_platform::cursor_position() else {
        return;
    };
    let Ok(monitors) = nc_platform::monitors() else {
        return;
    };
    let Some(monitor) = monitors
        .iter()
        .find(|monitor| point_in_rect(monitor.bounds, x as f64, y as f64))
    else {
        return;
    };
    let work = if state.settings.read().edge_inset {
        monitor.work_area
    } else {
        monitor.bounds
    };
    let direction = nearest_edge_direction(work, x as f64, y as f64);
    state.input.lock().edge_detector.block(direction);
}

fn nearest_edge_direction(work: nc_platform::RectI, x: f64, y: f64) -> Direction {
    let left = x - work.x as f64;
    let right = work.right() as f64 - x;
    let up = y - work.y as f64;
    let down = work.bottom() as f64 - y;
    let minimum = left.min(right).min(up).min(down);
    if minimum == left {
        Direction::Left
    } else if minimum == right {
        Direction::Right
    } else if minimum == up {
        Direction::Up
    } else {
        Direction::Down
    }
}

pub async fn toggle_stage(state: &Arc<AppState>) {
    let snapshot = state.snapshot();
    if snapshot.stage_visible || snapshot.ui_state == UiState::Stage {
        hide_stage(state).await;
    } else {
        summon_stage(state).await;
    }
}

pub async fn set_mode(state: &Arc<AppState>, mode: Mode) {
    dispatch_session(state, SessionEvent::SetMode { mode }).await;
}

pub async fn toggle_mode(state: &Arc<AppState>) {
    dispatch_session(state, SessionEvent::ToggleMode).await;
}

pub async fn start_discovery_command(state: &Arc<AppState>) {
    dispatch_session(state, SessionEvent::StartDiscovery).await;
}

pub async fn probe_discovery(state: &Arc<AppState>) -> Result<(), String> {
    if state.discovery.lock().is_none() {
        start_discovery(state);
    }
    let service = state.discovery.lock().clone();
    let Some(service) = service else {
        return Err("设备发现未启动".to_owned());
    };
    service.probe().await.map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn stop_discovery_command(state: &Arc<AppState>) {
    dispatch_session(state, SessionEvent::StopDiscovery).await;
}

pub async fn disconnect_command(state: &Arc<AppState>) {
    state.user_disconnect.store(true, Ordering::SeqCst);
    dispatch_session(state, SessionEvent::UserAbort).await;
    cleanup_connection(state).await;
}

pub async fn enter_control_panel(state: &Arc<AppState>) {
    dispatch_session(
        state,
        SessionEvent::UiStateChanged {
            ui_state: UiState::ControlPanel,
        },
    )
    .await;
}

pub async fn enter_background(state: &Arc<AppState>) {
    dispatch_session(
        state,
        SessionEvent::UiStateChanged {
            ui_state: UiState::Background,
        },
    )
    .await;
}

pub async fn shutdown(state: &Arc<AppState>) {
    cleanup_connection(state).await;
    close_all_links(state);
    stop_discovery(state);
    state.input.lock().stop();
    let streams: Vec<_> = state
        .media
        .outgoing
        .lock()
        .drain()
        .map(|(_, stream)| stream)
        .collect();
    for mut stream in streams {
        stream.stop_blocking();
    }
}

