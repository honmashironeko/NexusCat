use crate::ipc::{self, ControlOsdView, EdgeHaloView, EdgeOsdView};
use crate::runtime::{AppState, SessionRole};
use nc_core::{Direction, Mode};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

pub const OSD_WINDOW: &str = "osd";
pub const HALO_WINDOW: &str = "edge-halo";
pub const CONTROL_OSD_WINDOW: &str = "control-osd";

#[cfg(target_os = "macos")]
static CONTROL_OSD_WINDOW_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn control_osd_window_id() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let id = CONTROL_OSD_WINDOW_ID.load(Ordering::SeqCst);
        (id != 0).then_some(id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

pub fn refresh_control_osd_window_id(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        if let Some(window) = app.get_webview_window(CONTROL_OSD_WINDOW) {
            record_control_osd_window_id(&window);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

#[cfg(target_os = "macos")]
fn record_control_osd_window_id(window: &WebviewWindow) {
    use std::ffi::{c_char, c_void};

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_msgSend(receiver: *mut c_void, selector: *const c_void) -> isize;
        fn sel_registerName(name: *const c_char) -> *const c_void;
    }

    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    if ns_window.is_null() {
        return;
    }
    let selector = unsafe { sel_registerName(c"windowNumber".as_ptr()) };
    let number = unsafe { objc_msgSend(ns_window.cast(), selector) };
    if let Ok(id) = u32::try_from(number) {
        if id != 0 {
            CONTROL_OSD_WINDOW_ID.store(id, Ordering::SeqCst);
        }
    }
}
const OSD_WIDTH: f64 = 300.0;
const OSD_HEIGHT: f64 = 168.0;
const OSD_MARGIN: f64 = 18.0;
const OSD_HIDE_MS: u64 = 2400;
const CONTROL_OSD_WIDTH: f64 = 560.0;
const CONTROL_OSD_HEIGHT: f64 = 220.0;
const CONTROL_OSD_IDLE_MS: u64 = 10_000;
const CONTROL_OSD_TICK_MS: u64 = 250;

pub fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(app, OSD_WINDOW, WebviewUrl::App("index.html".into()))
        .title("NexusCat OSD")
        .inner_size(OSD_WIDTH, OSD_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    let _ = window.set_ignore_cursor_events(true);
    Ok(window)
}

pub fn create_halo(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(app, HALO_WINDOW, WebviewUrl::App("index.html".into()))
        .title("NexusCat Edge")
        .inner_size(1.0, 1.0)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    let _ = window.set_ignore_cursor_events(true);
    Ok(window)
}

pub fn create_control(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(
        app,
        CONTROL_OSD_WINDOW,
        WebviewUrl::App("index.html".into()),
    )
    .title("NexusCat Control OSD")
    .inner_size(CONTROL_OSD_WIDTH, CONTROL_OSD_HEIGHT)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .focused(false)
    .focusable(false)
    .visible(false)
    .build()?;
    let _ = window.set_ignore_cursor_events(true);
    if let Err(error) = window.set_content_protected(true) {
        tracing::warn!(%error, "控制权 OSD 无法设置采集排除");
    }
    #[cfg(target_os = "macos")]
    record_control_osd_window_id(&window);
    Ok(window)
}

pub fn direction_key(direction: Direction) -> &'static str {
    match direction {
        Direction::Left => "left",
        Direction::Right => "right",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

fn point_in_monitor(monitor: &nc_platform::MonitorInfo, x: f64, y: f64) -> bool {
    let bounds = monitor.bounds;
    x >= bounds.x as f64
        && x <= bounds.right() as f64
        && y >= bounds.y as f64
        && y <= bounds.bottom() as f64
}

fn monitor_for_cursor() -> Option<nc_platform::MonitorInfo> {
    let monitors = nc_platform::monitors().unwrap_or_default();
    let cursor = nc_platform::cursor_position().ok();
    cursor
        .and_then(|(x, y)| {
            monitors
                .iter()
                .find(|monitor| point_in_monitor(monitor, x as f64, y as f64))
                .cloned()
        })
        .or_else(|| monitors.iter().find(|monitor| monitor.primary).cloned())
        .or_else(|| monitors.first().cloned())
}

fn monitor_ui_scale(monitor: &nc_platform::MonitorInfo) -> f64 {
    #[cfg(target_os = "macos")]
    {
        monitor_coordinate_scale(monitor)
    }
    #[cfg(not(target_os = "macos"))]
    {
        (f64::from(monitor.dpi) / 96.0).clamp(0.5, 4.0)
    }
}

fn monitor_coordinate_scale(monitor: &nc_platform::MonitorInfo) -> f64 {
    #[cfg(target_os = "macos")]
    {
        point_pixel_scale(monitor.bounds.width, monitor.pixel_width)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = monitor;
        1.0
    }
}

#[cfg(target_os = "macos")]
fn point_pixel_scale(point_extent: u32, pixel_extent: u32) -> f64 {
    if point_extent == 0 || pixel_extent == 0 {
        return 1.0;
    }
    (f64::from(pixel_extent) / f64::from(point_extent)).clamp(1.0, 4.0)
}

fn rect_to_physical(rect: nc_platform::RectI, scale: f64) -> nc_platform::RectI {
    nc_platform::RectI::new(
        (f64::from(rect.x) * scale).round() as i32,
        (f64::from(rect.y) * scale).round() as i32,
        (f64::from(rect.width) * scale).round().max(1.0) as u32,
        (f64::from(rect.height) * scale).round().max(1.0) as u32,
    )
}

pub(crate) fn monitor_work_area() -> Option<(nc_platform::RectI, f64)> {
    let monitor = monitor_for_cursor()?;
    let work = rect_to_physical(monitor.work_area, monitor_coordinate_scale(&monitor));
    Some((work, monitor_ui_scale(&monitor)))
}

fn nearest_direction(work: nc_platform::RectI, x: f64, y: f64) -> Direction {
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

fn osd_position(
    work: nc_platform::RectI,
    direction: Direction,
    width: i32,
    height: i32,
    margin: i32,
) -> (i32, i32) {
    let max_x = (work.right() - width).max(work.x);
    let max_y = (work.bottom() - height).max(work.y);
    let centered_x = work.x + (work.width as i32 - width) / 2;
    let centered_y = work.y + (work.height as i32 - height) / 2;
    let (x, y) = match direction {
        Direction::Left => (work.x + margin, centered_y),
        Direction::Right => (work.right() - width - margin, centered_y),
        Direction::Up => (centered_x, work.y + margin),
        Direction::Down => (centered_x, work.bottom() - height - margin),
    };
    (x.clamp(work.x, max_x), y.clamp(work.y, max_y))
}

fn schedule_hide(state: &Arc<AppState>) {
    let generation = state.osd_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app = state.app.clone();
    let state = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(OSD_HIDE_MS)).await;
        if state.osd_generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if let Some(window) = app.get_webview_window(OSD_WINDOW) {
            let _ = window.hide();
        }
    });
}

fn primary_monitor() -> Option<nc_platform::MonitorInfo> {
    let monitors = nc_platform::monitors().unwrap_or_default();
    monitors
        .iter()
        .find(|monitor| monitor.primary)
        .or_else(|| monitors.first())
        .cloned()
}

fn centered_position(bounds: nc_platform::RectI, width: i32, height: i32) -> (i32, i32) {
    let x = bounds.x + (bounds.width as i32 - width).max(0) / 2;
    let y = bounds.y + (bounds.height as i32 - height).max(0) / 2;
    (x, y)
}

fn control_osd_context_active(state: &AppState) -> bool {
    let snapshot = state.snapshot();
    let mode = state.session_peer_mode.read().unwrap_or(snapshot.mode);
    *state.session_role.read() == Some(SessionRole::Target)
        && mode == Mode::Aggregation
        && snapshot.state.is_active()
        && !state.input.lock().remote_input_released
}

fn control_osd_payload(state: &AppState) -> ControlOsdView {
    let language = state.language();
    let device = state
        .peer()
        .map(|peer| peer.name)
        .unwrap_or_else(|| crate::i18n::generic_remote_device(language));
    let (kicker, title, subtitle, hint) = crate::i18n::control_osd(language, &device);
    ControlOsdView {
        kicker,
        title,
        subtitle,
        hint,
    }
}

fn show_control_osd_window(state: &AppState) {
    let Some(window) = state.app.get_webview_window(CONTROL_OSD_WINDOW) else {
        return;
    };
    let Some(monitor) = primary_monitor() else {
        return;
    };
    let scale = monitor_ui_scale(&monitor);
    let width = (CONTROL_OSD_WIDTH * scale).round().max(1.0) as i32;
    let height = (CONTROL_OSD_HEIGHT * scale).round().max(1.0) as i32;
    let bounds = rect_to_physical(monitor.bounds, monitor_coordinate_scale(&monitor));
    let (x, y) = centered_position(bounds, width, height);
    let _ = window.set_size(PhysicalSize::new(width as u32, height as u32));
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_content_protected(true);
    let _ = window.set_focusable(false);
    let _ = window.set_ignore_cursor_events(true);
    let _ = window.set_always_on_top(true);
    let _ = window.emit(ipc::EVENT_CONTROL_OSD, control_osd_payload(state));
    let _ = window.show();
    #[cfg(target_os = "macos")]
    record_control_osd_window_id(&window);
    let _ = window.set_focusable(false);
    let _ = window.set_ignore_cursor_events(true);
    let _ = window.set_always_on_top(true);
}

fn hide_control_osd_window(state: &AppState) {
    if let Some(window) = state.app.get_webview_window(CONTROL_OSD_WINDOW) {
        let _ = window.emit(ipc::EVENT_CONTROL_OSD, Option::<ControlOsdView>::None);
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        }
    }
}

pub fn start_control_osd(state: &Arc<AppState>) {
    if !control_osd_context_active(state)
        || state.app.get_webview_window(CONTROL_OSD_WINDOW).is_none()
    {
        return;
    }
    if state.control_osd_active.swap(true, Ordering::SeqCst) {
        return;
    }
    state
        .control_osd_last_input_ms
        .store(crate::notifications::now_ms(), Ordering::SeqCst);
    let generation = state.control_osd_generation.fetch_add(1, Ordering::SeqCst) + 1;
    show_control_osd_window(state);
    let state = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(CONTROL_OSD_TICK_MS)).await;
            if state.control_osd_generation.load(Ordering::SeqCst) != generation {
                break;
            }
            if !state.control_osd_active.load(Ordering::SeqCst) {
                break;
            }
            if !control_osd_context_active(&state) {
                stop_control_osd(&state);
                break;
            }
            if !state
                .app
                .get_webview_window(CONTROL_OSD_WINDOW)
                .and_then(|window| window.is_visible().ok())
                .unwrap_or(false)
            {
                let now = crate::notifications::now_ms();
                let last = state.control_osd_last_input_ms.load(Ordering::SeqCst);
                if now.saturating_sub(last) >= CONTROL_OSD_IDLE_MS {
                    show_control_osd_window(&state);
                }
            }
        }
    });
}

pub fn note_control_osd_local_input(state: &AppState) {
    state
        .control_osd_last_input_ms
        .store(crate::notifications::now_ms(), Ordering::SeqCst);
    if state.control_osd_active.load(Ordering::SeqCst) {
        hide_control_osd_window(state);
    }
}

pub fn stop_control_osd(state: &AppState) {
    state.control_osd_active.store(false, Ordering::SeqCst);
    state.control_osd_generation.fetch_add(1, Ordering::SeqCst);
    hide_control_osd_window(state);
}

pub fn show(state: &Arc<AppState>, direction: Option<Direction>, mut payload: EdgeOsdView) {
    let Some(window) = state.app.get_webview_window(OSD_WINDOW) else {
        return;
    };
    let Some((work, scale)) = monitor_work_area() else {
        return;
    };
    let direction = direction.unwrap_or_else(|| {
        let (x, y) = nc_platform::cursor_position().unwrap_or((work.x, work.y));
        nearest_direction(work, x as f64, y as f64)
    });
    payload.direction = direction_key(direction).to_string();
    let width = (OSD_WIDTH * scale).round().max(1.0) as u32;
    let height = (OSD_HEIGHT * scale).round().max(1.0) as u32;
    let margin = (OSD_MARGIN * scale).round() as i32;
    let (x, y) = osd_position(work, direction, width as i32, height as i32, margin);

    let _ = window.set_size(PhysicalSize::new(width, height));
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_focusable(false);
    let _ = window.set_ignore_cursor_events(true);
    let _ = window.set_always_on_top(true);
    let _ = window.emit(ipc::EVENT_EDGE_OSD, payload);
    let _ = window.show();
    let _ = window.set_focusable(false);
    let _ = window.set_ignore_cursor_events(true);
    let _ = window.set_always_on_top(true);
    schedule_hide(state);
}

pub fn show_halo(state: &Arc<AppState>, payload: EdgeHaloView, use_work_area: bool) {
    let Some(window) = state.app.get_webview_window(HALO_WINDOW) else {
        return;
    };
    let mut current = state.edge_halo.lock();
    if current.as_ref() == Some(&payload) && window.is_visible().unwrap_or(false) {
        return;
    }
    let Some(monitor) = monitor_for_cursor() else {
        return;
    };
    let scale = monitor_coordinate_scale(&monitor);
    let rect = rect_to_physical(
        if use_work_area {
            monitor.work_area
        } else {
            monitor.bounds
        },
        scale,
    );
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let _ = window.set_size(PhysicalSize::new(rect.width, rect.height));
    let _ = window.set_position(PhysicalPosition::new(rect.x, rect.y));
    let _ = window.set_focusable(false);
    let _ = window.set_ignore_cursor_events(true);
    let _ = window.set_always_on_top(true);
    let _ = window.emit(ipc::EVENT_EDGE_HALO, payload.clone());
    let _ = window.show();
    *current = Some(payload);
}

pub fn hide_halo(state: &Arc<AppState>) {
    if state.edge_halo.lock().take().is_none() {
        return;
    }
    if let Some(window) = state.app.get_webview_window(HALO_WINDOW) {
        let _ = window.hide();
    }
}

