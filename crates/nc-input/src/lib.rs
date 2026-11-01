use std::time::{SystemTime, UNIX_EPOCH};

use crossbeam_channel::Sender;
use nc_core::{InputEvent, KeyState, Modifiers, Result};

mod keymap;

#[cfg(windows)]
mod win32;

#[cfg(target_os = "macos")]
mod macos;

#[derive(Debug, Clone, PartialEq)]
pub struct CapturedInput {
    pub event: InputEvent,
    pub timestamp_us: u64,
    pub injected: bool,
    pub raw_relative: bool,
    pub local_only: bool,
}

pub trait InputCapture: Send {
    fn start(&mut self, sink: Sender<CapturedInput>) -> Result<()>;
    fn stop(&mut self);
    fn set_suppressed(&self, suppressed: bool);
    fn is_suppressed(&self) -> bool;
    fn is_running(&self) -> bool;
    fn set_relative_mode(&self, _enabled: bool) {}
    fn set_absolute_pointer(&self, _enabled: bool) {}
    fn latch_mouse_moves(&self, _direction: nc_core::Direction, _duration_ms: u64) {}
}

pub trait InputInjector: Send {
    fn inject(&mut self, event: &InputEvent) -> Result<()>;
    fn release_all(&mut self) -> Result<()>;
    fn reset_desktop(&mut self) {}
}

#[cfg(windows)]
pub fn create_input_capture() -> Result<Box<dyn InputCapture>> {
    Ok(Box::new(win32::WindowsInputCapture::new()))
}

#[cfg(target_os = "macos")]
pub fn create_input_capture() -> Result<Box<dyn InputCapture>> {
    Ok(Box::new(macos::MacosInputCapture::new()))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn create_input_capture() -> Result<Box<dyn InputCapture>> {
    Err(nc_core::NexusError::platform(
        "当前平台不支持输入捕获（仅支持 Windows 与 macOS）",
    ))
}

#[cfg(windows)]
pub fn create_input_injector() -> Result<Box<dyn InputInjector>> {
    Ok(Box::new(win32::WindowsInputInjector::new()))
}

#[cfg(target_os = "macos")]
pub fn create_input_injector() -> Result<Box<dyn InputInjector>> {
    Ok(Box::new(macos::MacosInputInjector::new()?))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn create_input_injector() -> Result<Box<dyn InputInjector>> {
    Err(nc_core::NexusError::platform(
        "当前平台不支持输入注入（仅支持 Windows 与 macOS）",
    ))
}

pub fn is_escape_hotkey(event: &InputEvent, modifiers: Modifiers) -> bool {
    if !modifiers.control() || !modifiers.alt() {
        return false;
    }

    matches!(
        event,
        InputEvent::Key {
            state: KeyState::Down,
            key_code: 0x29,
            ..
        }
    )
}

pub fn is_escape_key_down(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Key {
            state: KeyState::Down,
            key_code: 0x29,
            modifiers,
            ..
        } if !modifiers.control() && !modifiers.alt()
    )
}

pub fn is_escape_key_up(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Key {
            state: KeyState::Up,
            key_code: 0x29,
            ..
        }
    )
}

pub fn escape_release_pressed(captured: &CapturedInput) -> bool {
    if captured.injected {
        return false;
    }

    match &captured.event {
        InputEvent::Key { modifiers, .. } => is_escape_hotkey(&captured.event, *modifiers),
        _ => false,
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn unix_timestamp_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64
}

