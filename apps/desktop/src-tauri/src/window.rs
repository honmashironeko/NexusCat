use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow};

pub const MAIN_WINDOW: &str = "main";
pub const STAGE_WINDOW: &str = "stage";
const CAPSULE_WIDTH: f64 = 420.0;
const CAPSULE_HEIGHT: f64 = 322.0;
const PANEL_WIDTH: f64 = 1180.0;
const PANEL_HEIGHT: f64 = 760.0;
#[cfg(windows)]
const CORNER_RADIUS: f64 = 28.0;

#[cfg(windows)]
fn apply_rounded_region(window: &WebviewWindow) -> tauri::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn, HGDIOBJ};

    let hwnd = window.hwnd()?;
    let size = window.inner_size()?;
    let scale = window.scale_factor()?;
    if size.width == 0 || size.height == 0 {
        return Ok(());
    }
    let radius = (CORNER_RADIUS * scale).round().max(1.0) as i32;
    let region = unsafe {
        CreateRoundRectRgn(
            0,
            0,
            size.width as i32 + 1,
            size.height as i32 + 1,
            radius * 2,
            radius * 2,
        )
    };
    if region.is_invalid() {
        return Ok(());
    }
    let applied = unsafe { SetWindowRgn(HWND(hwnd.0), Some(region), true) };
    if applied == 0 {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn apply_rounded_region(_window: &WebviewWindow) -> tauri::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn clear_rounded_region(window: &WebviewWindow) -> tauri::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::SetWindowRgn;
    let hwnd = window.hwnd()?;
    unsafe {
        SetWindowRgn(HWND(hwnd.0), None, true);
    }
    Ok(())
}

#[cfg(not(windows))]
fn clear_rounded_region(_window: &WebviewWindow) -> tauri::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn unhide_app(app: &AppHandle) {
    let _ = app.show();
}

#[cfg(not(target_os = "macos"))]
fn unhide_app(_app: &AppHandle) {}

pub fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(MAIN_WINDOW)
}

pub fn stage_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(STAGE_WINDOW)
}

pub fn ensure_stage_window(app: &AppHandle) -> tauri::Result<(WebviewWindow, bool)> {
    if let Some(window) = stage_window(app) {
        return Ok((window, false));
    }
    let window =
        tauri::WebviewWindowBuilder::new(app, STAGE_WINDOW, WebviewUrl::App("index.html".into()))
            .title("NexusCat")
            .decorations(false)
            .transparent(false)
            .shadow(false)
            .resizable(false)
            .skip_taskbar(true)
            .always_on_top(true)
            .visible(false)
            .focused(false)
            .background_color(tauri::window::Color(4, 7, 11, 255))
            .build()?;
    Ok((window, true))
}

pub fn install_stage_close_handler<F>(window: &WebviewWindow, on_close: F)
where
    F: Fn() + Send + 'static,
{
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            on_close();
        }
    });
}

pub fn hide_main(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = main_window(app) {
        window.hide()?;
    }
    Ok(())
}

pub fn show_main(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = main_window(app) {
        window.show()?;
        window.unminimize()?;
        window.set_focus()?;
    }
    Ok(())
}

pub fn show_control_panel(app: &AppHandle) -> tauri::Result<()> {
    unhide_app(app);
    let Some(window) = main_window(app) else {
        return Ok(());
    };
    if window.is_fullscreen().unwrap_or(false) {
        let _ = window.set_fullscreen(false);
    }
    let _ = window.set_always_on_top(false);
    let _ = apply_rounded_region(&window);
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    let _ = apply_rounded_region(&window);
    hide_stage(app);
    let _ = window.set_focus();
    Ok(())
}

pub fn hide_stage(app: &AppHandle) {
    if let Some(window) = stage_window(app) {
        let _ = window.hide();
        let _ = window.set_fullscreen(false);
        let _ = window.set_always_on_top(false);
    }
}

pub fn set_control_panel_size(
    app: &AppHandle,
    expanded: bool,
    scale_factor: f64,
    available_width: f64,
    available_height: f64,
) -> tauri::Result<()> {
    let Some(window) = main_window(app) else {
        return Ok(());
    };
    if window.is_fullscreen().unwrap_or(false) {
        return Ok(());
    }
    let (preferred_width, preferred_height) = if expanded {
        (PANEL_WIDTH, PANEL_HEIGHT)
    } else {
        (CAPSULE_WIDTH, CAPSULE_HEIGHT)
    };
    let width = preferred_width.min((available_width - 16.0).max(360.0));
    let height = preferred_height.min((available_height - 16.0).max(300.0));
    let scale_factor = scale_factor.clamp(0.5, 4.0);
    window.set_size(tauri::PhysicalSize::new(
        (width * scale_factor).round() as u32,
        (height * scale_factor).round() as u32,
    ))?;
    window.center()?;
    let _ = apply_rounded_region(&window);
    Ok(())
}

pub fn show_stage(app: &AppHandle) -> tauri::Result<()> {
    unhide_app(app);
    let Some(window) = stage_window(app) else {
        return Ok(());
    };
    let _ = clear_rounded_region(&window);
    window.set_always_on_top(true)?;
    window.unminimize()?;
    window.show()?;
    window.set_focus()?;
    Ok(())
}

pub fn request_attention(app: &AppHandle) {
    if let Some(window) = main_window(app) {
        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
    }
}

pub fn hide_to_background(app: &AppHandle) -> tauri::Result<()> {
    hide_stage(app);
    if let Some(window) = main_window(app) {
        window.hide()?;
    }
    Ok(())
}

pub fn install_close_handler<F>(app: &AppHandle, quitting: Arc<AtomicBool>, on_close: F)
where
    F: Fn() + Send + 'static,
{
    let Some(window) = main_window(app) else {
        return;
    };
    let region_window = window.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            if quitting.load(Ordering::SeqCst) {
                return;
            }
            api.prevent_close();
            on_close();
        }
        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
            let covered = region_window.is_fullscreen().unwrap_or(false)
                || region_window.is_maximized().unwrap_or(false);
            if covered {
                let _ = clear_rounded_region(&region_window);
            } else {
                let _ = apply_rounded_region(&region_window);
            }
        }
        _ => {}
    });
}

pub fn quit(app: &AppHandle, quitting: &AtomicBool) {
    quitting.store(true, Ordering::SeqCst);
    app.exit(0);
}
