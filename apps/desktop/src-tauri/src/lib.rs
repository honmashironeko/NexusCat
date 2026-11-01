pub mod actions;
#[cfg(windows)]
pub mod autostart;
pub mod commands;
pub mod config;
pub mod elevation;
pub mod file_drop;
pub mod i18n;
#[cfg(windows)]
pub mod input_service;
pub mod ipc;
pub mod media_bridge;
pub mod media_runtime;
pub mod notifications;
pub mod orchestrator;
pub mod osd;
pub mod permissions;
pub mod runtime;
pub mod shortcuts;
pub mod startup;
pub mod tray;
pub mod window;

use actions::UiAction;
use media_bridge::MediaBridge;
use nc_topology::Layout;
use runtime::{AppEvent, AppState, InputRuntime, MediaRuntime, UpdateRuntime};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Manager;

pub fn handle_file_drop_broker_cli_mode() -> bool {
    #[cfg(windows)]
    {
        file_drop::handle_broker_cli_mode()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    startup::install_panic_hook();
    let test_mode = std::env::var("NEXUSCAT_TEST")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if let Err(detail) = startup::check_webview2() {
        startup::fatal_with_link(
            "缺少 WebView2 运行时",
            &detail,
            startup::WEBVIEW2_DOWNLOAD_URL,
        );
    }

    let (paths, path_warning) = startup::resolve_paths(test_mode);
    let first_run = !paths.settings_file.exists();
    let settings = config::load_settings(&paths);
    let identity = Arc::new(
        config::load_or_create_identity(&paths)
            .unwrap_or_else(|error| {
                startup::fatal(
                    "NexusCat 无法读取设备身份",
                    &format!(
                        "身份文件：{}\n\n原因：{error}\n\n请检查该文件是否被其他进程占用，或删除后重试。",
                        paths.identity_file.display()
                    ),
                )
            }),
    );
    let certificate = Arc::new(
        config::load_or_create_certificate(&paths, &identity).unwrap_or_else(|error| {
            startup::fatal(
                "NexusCat 无法生成 TLS 证书",
                &format!(
                    "证书目录：{}\n\n原因：{error}\n\n请检查当前用户对该目录的写权限。",
                    paths.data_dir.display()
                ),
            )
        }),
    );
    let trust = Arc::new(std::sync::RwLock::new(
        config::load_trust_store(&paths).unwrap_or_else(|error| {
            tracing::warn!(%error, "信任存储损坏，使用空存储");
            nc_crypto::TrustStore::new()
        }),
    ));
    let quitting = Arc::new(AtomicBool::new(false));

    let mut builder = tauri::Builder::default();
    if !test_mode {
        builder = builder.plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _cwd| {
                let _ = window::show_control_panel(app);
            },
        ));
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_plugin_macos_fps::init());
    }
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(move |app| {
            let handle = app.handle().clone();
            let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
            if let Ok(guard) = nc_observability::init_logging(&paths.logs_dir, &log_level) {
                Box::leak(Box::new(guard));
            }
            if let Some(warning) = &path_warning {
                tracing::warn!(%warning, "数据目录已回退");
            }
            let bridge = tauri::async_runtime::block_on(MediaBridge::start())?;
            let bridge_info = ipc::MediaBridgeInfo {
                port: bridge.port(),
                token: bridge.token().to_string(),
            };
            let (control_listener, media_socket, port_warning) =
                tauri::async_runtime::block_on(async {
                    let (listener, warning) =
                        startup::bind_control_listener(settings.control_port).await?;
                    let socket = Arc::new(tokio::net::UdpSocket::bind(("0.0.0.0", 0)).await?);
                    Ok::<_, std::io::Error>((listener, socket, warning))
                })?;
            if let Some(warning) = &port_warning {
                tracing::warn!(%warning, "控制端口已回退");
            }
            let control_port = control_listener
                .local_addr()
                .map(|address| address.port())
                .unwrap_or(settings.control_port);
            let media =
                MediaRuntime::new(Arc::clone(&media_socket), nc_media::MediaConfig::default())?;
            let media_port = media.media_port;
            let video_capabilities = runtime::VideoCapabilities::detect();
            let device_info = orchestrator::build_device_info(
                &identity,
                &settings.device_name,
                control_port,
                media_port,
                &video_capabilities,
            );

            let (action_tx, action_rx) = tokio::sync::mpsc::unbounded_channel::<UiAction>();
            let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
            let input = InputRuntime::default();
            let clipboard = Some(nc_clipboard::ClipboardWatcher::new(Duration::from_millis(
                500,
            )));
            let session = orchestrator::build_session(&settings);
            let snapshot = session.snapshot();
            let permissions = permissions::report();
            let update_manager =
                nc_update::UpdateManager::new(paths.local_dir.clone(), env!("CARGO_PKG_VERSION"))?;
            let update = UpdateRuntime::new(update_manager);

            let state = Arc::new(AppState {
                app: handle.clone(),
                paths: paths.clone(),
                test_mode,
                started: Instant::now(),
                settings: parking_lot::RwLock::new(settings.clone()),
                identity: Arc::clone(&identity),
                certificate: Arc::clone(&certificate),
                trust: Arc::clone(&trust),
                layout: parking_lot::RwLock::new(Layout::default()),
                monitors: parking_lot::RwLock::new(nc_platform::monitors().unwrap_or_default()),
                session: parking_lot::Mutex::new(session),
                snapshot: parking_lot::RwLock::new(snapshot),
                devices: parking_lot::RwLock::new(std::collections::HashMap::new()),
                discovery: parking_lot::Mutex::new(None),
                notifications: parking_lot::Mutex::new(notifications::NotificationCenter::new()),
                metrics: Arc::new(nc_observability::MetricsRegistry::new()),
                clock_offsets: parking_lot::Mutex::new(std::collections::HashMap::new()),
                bridge: Arc::new(bridge),
                bridge_info,
                media,
                last_stream_request: parking_lot::Mutex::new(None),
                input: parking_lot::Mutex::new(input),
                transfers: parking_lot::Mutex::new(Default::default()),
                shared_sync: parking_lot::Mutex::new(Default::default()),
                update: parking_lot::Mutex::new(update),
                links: parking_lot::Mutex::new(std::collections::HashMap::new()),
                connecting_links: parking_lot::Mutex::new(std::collections::HashSet::new()),
                session_peer: parking_lot::RwLock::new(None),
                session_role: parking_lot::RwLock::new(None),
                session_peer_mode: parking_lot::RwLock::new(None),
                group_host: parking_lot::RwLock::new(None),
                pending_session: parking_lot::RwLock::new(None),
                pending_remote_entry: parking_lot::Mutex::new(None),
                switching_target: AtomicBool::new(false),
                pending_session_started: parking_lot::Mutex::new(None),
                pending_session_retries: std::sync::atomic::AtomicU8::new(0),
                pending_takeover: parking_lot::Mutex::new(None),
                registered_shortcuts: parking_lot::Mutex::new(Vec::new()),
                aux_controls: parking_lot::Mutex::new(std::collections::HashMap::new()),
                pairing: parking_lot::Mutex::new(None),
                actions: action_tx.clone(),
                events: event_tx.clone(),
                tray: parking_lot::RwLock::new(None),
                quitting: Arc::clone(&quitting),
                webcodecs: parking_lot::RwLock::new(ipc::WebCodecsProbe::default()),
                video_capabilities,
                device_info: parking_lot::RwLock::new(device_info),
                control_listener_port: control_port,
                media_port,
                session_generation: std::sync::atomic::AtomicU64::new(0),
                osd_generation: std::sync::atomic::AtomicU64::new(0),
                drop_osd_generation: std::sync::atomic::AtomicU64::new(0),
                drop_osd_visible: AtomicBool::new(false),
                file_drop_hover: parking_lot::Mutex::new(None),
                file_drop_cards: parking_lot::Mutex::new(None),
                control_osd_generation: std::sync::atomic::AtomicU64::new(0),
                control_osd_last_input_ms: std::sync::atomic::AtomicU64::new(0),
                control_osd_active: std::sync::atomic::AtomicBool::new(false),
                edge_halo: parking_lot::Mutex::new(None),
                reconnect_task: parking_lot::Mutex::new(None),
                clipboard_last_hash: parking_lot::RwLock::new(None),
                clipboard: parking_lot::Mutex::new(clipboard),
                pending_connect: parking_lot::RwLock::new(None),
                permissions: parking_lot::RwLock::new(permissions),
                first_frame_wait: parking_lot::Mutex::new(None),
                user_disconnect: AtomicBool::new(false),
                reconnect_attempt: std::sync::atomic::AtomicU64::new(0),
                input_seq: std::sync::atomic::AtomicU64::new(0),
                adaptive: parking_lot::Mutex::new(runtime::AdaptiveState::default()),
                secure_desktop: AtomicBool::new(false),
                secure_desktop_service: AtomicBool::new(false),
                main_pending_surface_render: AtomicBool::new(false),
                main_surface_ready: AtomicBool::new(false),
                stage_pending_surface_render: AtomicBool::new(false),
                stage_surface_ready: AtomicBool::new(false),
            });

            app.manage(Arc::clone(&state));
            orchestrator::notify_input_service_update(&state);
            runtime::spawn_input_capture(Arc::clone(&state));
            let initial_layout = orchestrator::load_initial_layout(&state);
            *state.layout.write() = initial_layout.clone();
            state.session.lock().set_layout(initial_layout.clone());
            ipc::emit(&handle, ipc::EVENT_LAYOUT_CHANGED, initial_layout);
            let close_actions = state.actions.clone();
            window::install_close_handler(&handle, Arc::clone(&quitting), move || {
                let _ = close_actions.send(UiAction::HideToBackground);
            });

            if !test_mode {
                use tauri_plugin_notification::NotificationExt;
                let action_state = Arc::clone(&state);
                if let Err(error) = handle.notification().on_action(move |performed| {
                    if let Some(kind) =
                        nc_core::NotificationActionKind::from_action_id(performed.action_id())
                    {
                        let _ = action_state.actions.send(UiAction::NotificationAction {
                            kind,
                            payload: None,
                        });
                    }
                }) {
                    tracing::warn!(%error, "注册系统通知动作监听失败");
                }
                match tray::create_tray(&handle, action_tx.clone()) {
                    Ok(tray) => orchestrator::set_tray(&state, tray),
                    Err(error) => {
                        tracing::warn!(%error, "创建托盘失败，改为显示控制层");
                        let _ = window::show_control_panel(&handle);
                    }
                }
                if let Err(error) = osd::create(&handle) {
                    tracing::warn!(%error, "创建 OSD 窗口失败");
                }
                if let Err(error) = osd::create_halo(&handle) {
                    tracing::warn!(%error, "创建边缘光环窗口失败");
                }
                if let Err(error) = osd::create_control(&handle) {
                    tracing::warn!(%error, "创建控制权 OSD 窗口失败");
                }
                if let Err(error) = file_drop::create(&handle) {
                    tracing::warn!(%error, "创建文件拖放浮层失败");
                }
                file_drop::install_main_drag_handler(&handle, Arc::clone(&state));
                file_drop::install_osd_drop_handler(&handle, Arc::clone(&state));
                file_drop::install_desktop_drag_watcher(Arc::clone(&state));
                file_drop::start_broker_if_elevated(&state);
                #[cfg(windows)]
                input_service::start();
            }

            #[cfg(windows)]
            if !test_mode {
                use tauri_plugin_autostart::ManagerExt;
                let _ = app.autolaunch().disable();
                if let Err(error) = autostart::remove_unsafe_task() {
                    tracing::warn!(%error, "清理不安全的登录自启任务失败");
                }
                let autostart_result = if settings.autostart && !autostart::supported() {
                    tracing::warn!(
                        "当前程序不在 Program Files，跳过登录自启计划任务；便携版请手动启动"
                    );
                    Ok(())
                } else {
                    autostart::sync_startup(settings.autostart)
                };
                if let Err(error) = autostart_result {
                    tracing::warn!(%error, "同步登录自启计划任务失败");
                    orchestrator::notify_autostart_failed(&state, &error);
                }
            }
            #[cfg(not(windows))]
            {
                use tauri_plugin_autostart::ManagerExt;
                if settings.autostart && !test_mode {
                    if let Err(error) = app.autolaunch().enable() {
                        tracing::warn!(%error, "启用登录自启失败");
                    }
                } else if !settings.autostart && !test_mode {
                    let _ = app.autolaunch().disable();
                }
            }

            if !test_mode {
                if let Err(error) =
                    shortcuts::register_initial(&handle, &settings.shortcuts, Arc::clone(&state))
                {
                    let delays: Vec<u64> = if error.is_conflict() {
                        vec![1_000]
                    } else {
                        vec![1_000, 3_000, 10_000]
                    };
                    tracing::warn!(
                        error = error.message(),
                        "注册全局快捷键失败，将在启动后重试"
                    );
                    let retry_handle = handle.clone();
                    let retry_state = Arc::clone(&state);
                    tauri::async_runtime::spawn(async move {
                        let mut last_error = error;
                        let mut registered = false;
                        for delay_ms in delays {
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                            let settings = retry_state.settings.read().shortcuts.clone();
                            match shortcuts::register_initial(
                                &retry_handle,
                                &settings,
                                Arc::clone(&retry_state),
                            ) {
                                Ok(()) => {
                                    tracing::info!("全局快捷键重试注册成功");
                                    registered = true;
                                    break;
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        error = error.message(),
                                        "全局快捷键重试注册失败"
                                    );
                                    last_error = error;
                                }
                            }
                        }
                        if !registered {
                            orchestrator::notify_shortcut_failure(
                                &retry_state,
                                last_error.message(),
                            );
                        }
                    });
                }
            }

            let orchestrator_state = Arc::clone(&state);
            let timer_resolution = nc_platform::TimerResolutionGuard::acquire();
            tauri::async_runtime::spawn(async move {
                let _timer_resolution = timer_resolution;
                orchestrator::run(orchestrator_state, action_rx, event_rx).await;
            });
            orchestrator::spawn_accept_loop(Arc::clone(&state), control_listener);

            let discovery_state = Arc::clone(&state);
            tauri::async_runtime::spawn(async move {
                orchestrator::post_session(
                    &discovery_state,
                    nc_session::SessionEvent::StartDiscovery,
                )
                .await;
            });

            let show_initial = test_mode || first_run;
            let ui_state = if show_initial {
                nc_core::UiState::ControlPanel
            } else {
                nc_core::UiState::Background
            };
            let ui_state_value = state.clone();
            if show_initial {
                let _ = window::show_control_panel(&handle);
            }
            tauri::async_runtime::spawn(async move {
                orchestrator::set_ui_state(&ui_state_value, ui_state).await;
            });

            #[cfg(target_os = "macos")]
            if !test_mode {
                permissions::auto_request_missing_once(&handle, &paths);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_bootstrap,
            commands::get_state,
            commands::get_devices,
            commands::get_layout,
            commands::get_settings,
            commands::get_transfers,
            commands::get_notifications,
            commands::get_metrics,
            commands::get_permissions,
            commands::get_update_status,
            commands::get_media_bridge,
            commands::start_discovery,
            commands::stop_discovery,
            commands::probe_devices,
            commands::pair_device,
            commands::confirm_pairing,
            commands::cancel_pairing,
            commands::remove_trusted_device,
            commands::connect_device,
            commands::disconnect,
            commands::set_mode,
            commands::toggle_mode,
            commands::summon_stage,
            commands::hide_stage,
            commands::set_layout,
            commands::update_settings,
            commands::sync_shared_config,
            commands::send_files,
            commands::show_file_drop_osd,
            commands::hide_file_drop_osd,
            commands::set_file_drop_hover,
            commands::set_file_drop_cards,
            commands::get_input_service_status,
            commands::install_input_service,
            commands::uninstall_input_service,
            commands::cancel_transfer,
            commands::clear_finished_transfers,
            commands::open_downloads_folder,
            commands::mark_notification_read,
            commands::mark_all_notifications_read,
            commands::clear_notifications,
            commands::request_permission,
            commands::open_permission_settings,
            commands::push_update,
            commands::apply_update,
            commands::rollback_update,
            commands::show_control_panel,
            commands::retry_connection,
            commands::report_surface_ready,
            commands::hide_to_background,
            commands::set_shortcut_capture,
            commands::set_control_panel_size,
            commands::quit_app,
            commands::report_present_stats,
            commands::request_keyframe,
            commands::webcodecs_probe_result,
            commands::set_autostart,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| startup::fatal("NexusCat 启动失败", &format!("{error:#}")))
        .run(|app_handle, event| {
            #[cfg(target_os = "macos")]
            {
                if matches!(event, tauri::RunEvent::Reopen { .. }) {
                    let main_visible = app_handle
                        .get_webview_window(window::MAIN_WINDOW)
                        .and_then(|main| main.is_visible().ok())
                        .unwrap_or(false);
                    if !main_visible {
                        let shown = app_handle
                            .try_state::<Arc<AppState>>()
                            .is_some_and(|state| {
                                state.actions.send(UiAction::ShowControlPanel).is_ok()
                            });
                        if !shown {
                            let _ = window::show_control_panel(app_handle);
                        }
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (app_handle, event);
            }
        });
}
