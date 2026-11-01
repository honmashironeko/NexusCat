mod desktop_drag;

#[cfg(windows)]
mod broker;

use crate::ipc;
use crate::runtime::{AppEvent, AppState, BrokerEvent};
use crate::window;
use desktop_drag::DragSignal;
use nc_core::{DeviceId, DisplayNode, DisplayNodeId};
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::{
    AppHandle, DragDropEvent, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

pub const FILE_DROP_WINDOW: &str = "file-drop";
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 300.0;
const MARGIN: f64 = 18.0;
const DRAG_END_HIDE_MS: u64 = 300;
const SAFETY_HIDE_MS: u64 = 30_000;

pub(crate) fn handle_broker_cli_mode() -> bool {
    #[cfg(windows)]
    {
        broker::handle_cli_mode()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub(crate) fn start_broker_if_elevated(state: &Arc<AppState>) {
    #[cfg(windows)]
    broker::start_if_elevated(state);
    #[cfg(not(windows))]
    let _ = state;
}

fn broker_required() -> bool {
    crate::elevation::is_elevated()
}

fn broker_ready() -> bool {
    #[cfg(windows)]
    {
        broker::available()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window =
        WebviewWindowBuilder::new(app, FILE_DROP_WINDOW, WebviewUrl::App("index.html".into()))
            .title("NexusCat Drop")
            .inner_size(WIDTH, HEIGHT)
            .position(-32000.0, -32000.0)
            .resizable(false)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .focused(false)
            .focusable(false)
            .visible(true)
            .build()?;
    let _ = window.set_ignore_cursor_events(true);
    Ok(window)
}

pub fn install_main_drag_handler(app: &AppHandle, state: Arc<AppState>) {
    let Some(window) = window::main_window(app) else {
        return;
    };
    #[cfg(windows)]
    if crate::elevation::is_elevated() {
        tracing::info!(
            "进程以管理员权限运行，桌面拖放由 Medium 拖放代理接管；直接拖入主窗口仍受 UIPI 限制"
        );
    }
    window.on_window_event(move |event| {
        let WindowEvent::DragDrop(drag) = event else {
            return;
        };
        match drag {
            DragDropEvent::Enter { paths, .. } => {
                tracing::info!(files = paths.len(), "文件拖入主窗口");
                show(&state, Some(paths.len()));
            }
            DragDropEvent::Over { .. } => {}
            DragDropEvent::Leave => {}
            DragDropEvent::Drop { paths, .. } => {
                tracing::info!(files = paths.len(), "文件直接落在主窗口");
                hide(&state);
                let _ = state.events.send(crate::runtime::AppEvent::FileDropHint);
            }
            _ => {}
        }
    });
}

pub fn install_osd_drop_handler(app: &AppHandle, state: Arc<AppState>) {
    let Some(window) = app.get_webview_window(FILE_DROP_WINDOW) else {
        return;
    };
    window.on_window_event(move |event| {
        let WindowEvent::DragDrop(drag) = event else {
            return;
        };
        match drag {
            DragDropEvent::Enter { paths, .. } => {
                tracing::info!(files = paths.len(), "文件拖放浮层收到拖动进入");
            }
            DragDropEvent::Leave => {
                tracing::debug!("文件拖放浮层拖动离开");
            }
            DragDropEvent::Drop {
                paths, position, ..
            } => {
                let hover = state.file_drop_hover.lock().clone();
                let node_id = drop_node_at(&state, position.x, position.y).or(hover);
                tracing::info!(
                    files = paths.len(),
                    node = ?node_id,
                    x = position.x,
                    y = position.y,
                    "文件拖放浮层收到落点"
                );
                if !state.settings.read().file_transfer_enabled {
                    return;
                }
                let Some(node_id) = node_id else {
                    return;
                };
                let Some(target) = drop_device(&state, &node_id) else {
                    return;
                };
                let paths: Vec<std::path::PathBuf> = paths.to_vec();
                let state = Arc::clone(&state);
                tauri::async_runtime::spawn(async move {
                    hide(&state);
                    crate::orchestrator::send_files(&state, paths, Some(target)).await;
                });
            }
            DragDropEvent::Over { .. } => {}
            _ => {}
        }
    });
}

pub(crate) fn drop_device(state: &AppState, node_id: &str) -> Option<DeviceId> {
    let layout = state.layout.read();
    let id = DisplayNodeId(node_id.to_string());
    let node = layout.node(&id)?;
    (layout.is_active(&id) && !node.is_local).then_some(node.device_id)
}

fn drop_node_at(state: &AppState, x: f64, y: f64) -> Option<String> {
    let cards = state.file_drop_cards.lock();
    cards.as_ref().and_then(|cards| node_at_cards(cards, x, y))
}

pub(crate) fn node_at_position(state: &AppState, x: i32, y: i32) -> Option<String> {
    drop_node_at(state, x as f64, y as f64)
}

pub(crate) fn resolve_drop_node(state: &AppState, x: i32, y: i32) -> Option<String> {
    drop_node_at(state, x as f64, y as f64).or_else(|| state.file_drop_hover.lock().clone())
}

pub(crate) fn set_broker_hover(state: &AppState, node_id: Option<String>, file_count: Option<u32>) {
    if !state.drop_osd_visible.load(Ordering::SeqCst) {
        return;
    }
    let changed = {
        let mut hover = state.file_drop_hover.lock();
        if *hover != node_id {
            *hover = node_id.clone();
            true
        } else {
            false
        }
    };
    if !changed && file_count.is_none() {
        return;
    }
    let Some(window) = state.app.get_webview_window(FILE_DROP_WINDOW) else {
        return;
    };
    let _ = window.emit(
        ipc::EVENT_FILE_DROP_HOVER,
        ipc::FileDropHoverView {
            node_id,
            file_count,
        },
    );
}

fn node_at_cards(cards: &ipc::FileDropCardsView, x: f64, y: f64) -> Option<String> {
    let scale = if cards.scale.is_finite() && cards.scale > 0.0 {
        cards.scale
    } else {
        1.0
    };
    let local_x = x / scale;
    let local_y = y / scale;
    cards
        .cards
        .iter()
        .find(|card| {
            local_x >= card.x
                && local_x <= card.x + card.width
                && local_y >= card.y
                && local_y <= card.y + card.height
        })
        .map(|card| card.node_id.clone())
}

pub fn install_desktop_drag_watcher(state: Arc<AppState>) {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    desktop_drag::spawn(sender);
    tauri::async_runtime::spawn(async move {
        while let Some(signal) = receiver.recv().await {
            match signal {
                DragSignal::Started => {
                    if desktop_drop_ready(&state) {
                        tracing::info!("桌面拖动开始，显示文件拖放浮层");
                        show(&state, None);
                    } else {
                        tracing::debug!("桌面拖动开始，但没有可发送的远端屏幕");
                    }
                }
                DragSignal::Ended => {
                    tracing::debug!("桌面拖动结束");
                    schedule_hide(&state, DRAG_END_HIDE_MS);
                }
            }
        }
    });
}

fn desktop_drop_ready(state: &AppState) -> bool {
    if !state.settings.read().file_transfer_enabled {
        return false;
    }
    visible_nodes(state).iter().any(|node| !node.is_local)
}

pub fn show(state: &Arc<AppState>, file_count: Option<usize>) {
    let generation = state.drop_osd_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(window) = state.app.get_webview_window(FILE_DROP_WINDOW) else {
        tracing::warn!("文件拖放浮层窗口不存在");
        return;
    };
    let Some((work, scale)) = crate::osd::monitor_work_area() else {
        tracing::warn!("无法确定文件拖放浮层所在显示器");
        return;
    };
    let width = (WIDTH * scale).round().max(1.0) as u32;
    let height = (HEIGHT * scale).round().max(1.0) as u32;
    let margin = (MARGIN * scale).round() as i32;
    let (x, y) = drop_position(work, width as i32, height as i32, margin);

    let elevated_drag = broker_required();
    if elevated_drag && !broker_ready() {
        hide(state);
        let _ = state
            .events
            .send(AppEvent::FileDropBroker(BrokerEvent::Unavailable {
                reason: "文件拖放代理尚未就绪".into(),
            }));
        return;
    }

    let first_show = !state.drop_osd_visible.swap(true, Ordering::SeqCst);
    let _ = window.emit(ipc::EVENT_FILE_DROP_OSD, payload(state, file_count));
    let size_changed = window
        .inner_size()
        .map(|size| size.width != width || size.height != height)
        .unwrap_or(true);
    if size_changed {
        let _ = window.set_size(PhysicalSize::new(width, height));
    }
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_focusable(false);
    let _ = window.set_ignore_cursor_events(elevated_drag);
    let _ = window.set_always_on_top(true);
    if !window.is_visible().unwrap_or(false) {
        let _ = window.show();
    }
    #[cfg(windows)]
    if elevated_drag && !broker::show(x, y, width as i32, height as i32) {
        hide(state);
        let _ = state
            .events
            .send(AppEvent::FileDropBroker(BrokerEvent::Unavailable {
                reason: "无法把拖放代理覆盖到文件拖放浮层".into(),
            }));
        return;
    }
    tracing::info!(?file_count, first_show, "文件拖放浮层已显示");
    schedule_hide_for(state, SAFETY_HIDE_MS, generation);
}

pub fn hide(state: &AppState) {
    #[cfg(windows)]
    broker::hide();
    state.drop_osd_generation.fetch_add(1, Ordering::SeqCst);
    state.drop_osd_visible.store(false, Ordering::SeqCst);
    *state.file_drop_hover.lock() = None;
    *state.file_drop_cards.lock() = None;
    let Some(window) = state.app.get_webview_window(FILE_DROP_WINDOW) else {
        return;
    };
    let _ = window.set_ignore_cursor_events(true);
    let _ = window.emit(ipc::EVENT_FILE_DROP_OSD_HIDDEN, ());
}

fn schedule_hide(state: &Arc<AppState>, delay_ms: u64) {
    let generation = state.drop_osd_generation.load(Ordering::SeqCst);
    schedule_hide_for(state, delay_ms, generation);
}

fn schedule_hide_for(state: &Arc<AppState>, delay_ms: u64, generation: u64) {
    let state = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        if state.drop_osd_generation.load(Ordering::SeqCst) != generation {
            return;
        }
        hide(&state);
    });
}

fn payload(state: &AppState, file_count: Option<usize>) -> ipc::FileDropOsdView {
    ipc::FileDropOsdView {
        nodes: visible_nodes(state),
        file_count,
        enabled: state.settings.read().file_transfer_enabled,
        language: state.language(),
    }
}

fn visible_nodes(state: &AppState) -> Vec<DisplayNode> {
    let layout = state.layout.read();
    let connected: HashSet<DeviceId> = state
        .links
        .lock()
        .iter()
        .filter(|(_, link)| link.connected.load(Ordering::SeqCst))
        .map(|(device, _)| *device)
        .collect();
    layout
        .nodes
        .iter()
        .filter(|node| layout.is_active(&node.id))
        .filter(|node| node.is_local || connected.contains(&node.device_id))
        .cloned()
        .collect()
}

fn drop_position(work: nc_platform::RectI, width: i32, height: i32, margin: i32) -> (i32, i32) {
    let max_x = (work.right() - width).max(work.x);
    let max_y = (work.bottom() - height).max(work.y);
    let x = (work.right() - width - margin).clamp(work.x, max_x);
    let y = (work.bottom() - height - margin).clamp(work.y, max_y);
    (x, y)
}
