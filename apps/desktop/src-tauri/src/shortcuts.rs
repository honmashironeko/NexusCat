use crate::actions::UiAction;
use crate::runtime::AppState;
use nc_core::{Modifiers as InputModifiers, ShortcutSettings};
use std::sync::Arc;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

struct RegistrationFailure {
    action: UiAction,
    message: String,
    conflict: bool,
}

#[derive(Debug)]
pub struct RegistrationError {
    message: String,
    conflict: bool,
}

impl RegistrationError {
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn is_conflict(&self) -> bool {
        self.conflict
    }
}

#[derive(Debug, Clone)]
pub struct ShortcutEntry {
    pub action: UiAction,
    pub text: String,
    pub shortcut: Shortcut,
}

fn push_entry(
    entries: &mut Vec<ShortcutEntry>,
    label: &str,
    value: &str,
    action: UiAction,
) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    let shortcut: Shortcut = value
        .parse()
        .map_err(|error| format!("{label}快捷键“{value}”无效：{error}"))?;
    let primary_modifiers = Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER;
    if (shortcut.mods & primary_modifiers).is_empty() {
        return Err(format!("{label}快捷键至少需要 Ctrl / Alt / Super 之一"));
    }
    if entries
        .iter()
        .any(|existing| existing.shortcut.id() == shortcut.id())
    {
        return Err(format!("{label}快捷键与另一个动作重复"));
    }
    entries.push(ShortcutEntry {
        action,
        text: value.to_string(),
        shortcut,
    });
    Ok(())
}

pub fn entries(settings: &ShortcutSettings) -> Result<Vec<ShortcutEntry>, String> {
    let mut entries = Vec::new();
    push_entry(
        &mut entries,
        "模式切换",
        &settings.toggle_mode,
        UiAction::ToggleMode,
    )?;
    push_entry(
        &mut entries,
        "全屏",
        &settings.toggle_stage,
        UiAction::ToggleStage,
    )?;
    push_entry(
        &mut entries,
        "校准层",
        &settings.open_calibration,
        UiAction::ShowCalibration,
    )?;
    push_entry(
        &mut entries,
        "暂停远端控制",
        &settings.pause_remote,
        UiAction::ToggleRemotePause,
    )?;
    push_entry(
        &mut entries,
        "暂停边缘触发",
        &settings.toggle_edge,
        UiAction::ToggleEdge,
    )?;
    Ok(entries)
}

fn register(
    app: &AppHandle,
    entries: &[ShortcutEntry],
    state: &Arc<AppState>,
    registered: &mut Vec<Shortcut>,
) -> Vec<RegistrationFailure> {
    let manager = app.global_shortcut();
    let mut failures = Vec::new();
    for entry in entries {
        let action = entry.action.clone();
        let action_state = Arc::clone(state);
        let action_for_event = action.clone();
        if let Err(error) = manager.on_shortcut(entry.shortcut, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let _ = action_state.actions.send(action_for_event.clone());
            }
        }) {
            let detail = error.to_string();
            let conflict = is_conflict_error(&detail);
            failures.push(RegistrationFailure {
                message: failure_message(action_label(&action), &entry.text, conflict, &detail),
                action,
                conflict,
            });
        } else {
            registered.push(entry.shortcut);
        }
    }
    failures
}

fn is_conflict_error(error: &str) -> bool {
    error.to_ascii_lowercase().contains("already registered")
}

fn failure_message(label: &str, text: &str, conflict: bool, error: &str) -> String {
    if conflict {
        format!("{label}（{text}）已被其他程序占用")
    } else {
        format!("{label}（{text}）注册失败：{error}")
    }
}

fn failure_error(failures: &[RegistrationFailure]) -> RegistrationError {
    RegistrationError {
        message: failures
            .iter()
            .map(|failure| failure.message.clone())
            .collect::<Vec<_>>()
            .join("；"),
        conflict: !failures.is_empty() && failures.iter().all(|failure| failure.conflict),
    }
}

fn unregister_registered(app: &AppHandle, registered: &mut Vec<Shortcut>) {
    let manager = app.global_shortcut();
    for shortcut in registered.drain(..) {
        if let Err(error) = manager.unregister(shortcut) {
            tracing::debug!(
                %error,
                shortcut = %shortcut.into_string(),
                "注销快捷键时忽略错误"
            );
        }
    }
}

fn action_label(action: &UiAction) -> &'static str {
    match action {
        UiAction::ToggleMode => "模式切换",
        UiAction::ToggleStage => "全屏",
        UiAction::ShowCalibration => "校准层",
        UiAction::ToggleRemotePause => "暂停远端控制",
        UiAction::ToggleEdge => "暂停边缘触发",
        _ => "快捷键",
    }
}

fn blocks_update(previous: &ShortcutSettings, next: &ShortcutSettings, action: &UiAction) -> bool {
    shortcut_value(previous, action) != shortcut_value(next, action)
}

fn shortcut_value<'a>(settings: &'a ShortcutSettings, action: &UiAction) -> &'a str {
    match action {
        UiAction::ToggleMode => &settings.toggle_mode,
        UiAction::ToggleStage => &settings.toggle_stage,
        UiAction::ShowCalibration => &settings.open_calibration,
        UiAction::ToggleRemotePause => &settings.pause_remote,
        UiAction::ToggleEdge => &settings.toggle_edge,
        _ => "",
    }
}

pub fn suspend(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let mut registered = state.registered_shortcuts.lock();
    unregister_registered(app, &mut registered);
    Ok(())
}

pub fn refresh(
    app: &AppHandle,
    settings: &ShortcutSettings,
    state: Arc<AppState>,
) -> Result<(), String> {
    let entries = entries(settings)?;
    let mut registered = state.registered_shortcuts.lock();
    unregister_registered(app, &mut registered);
    for failure in register(app, &entries, &state, &mut registered) {
        tracing::warn!("{}", failure.message);
    }
    Ok(())
}

pub fn register_initial(
    app: &AppHandle,
    settings: &ShortcutSettings,
    state: Arc<AppState>,
) -> Result<(), RegistrationError> {
    let entries = entries(settings).map_err(|message| RegistrationError {
        message,
        conflict: false,
    })?;
    let mut registered = state.registered_shortcuts.lock();
    unregister_registered(app, &mut registered);
    let failures = register(app, &entries, &state, &mut registered);
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failure_error(&failures))
    }
}

pub fn apply(
    app: &AppHandle,
    previous: &ShortcutSettings,
    next: &ShortcutSettings,
    state: Arc<AppState>,
) -> Result<(), String> {
    if previous == next {
        return Ok(());
    }
    let next_entries = entries(next)?;
    let mut registered = state.registered_shortcuts.lock();
    unregister_registered(app, &mut registered);
    let failures = register(app, &next_entries, &state, &mut registered);
    let blocking: Vec<&RegistrationFailure> = failures
        .iter()
        .filter(|failure| blocks_update(previous, next, &failure.action))
        .collect();
    if !blocking.is_empty() {
        unregister_registered(app, &mut registered);
        if let Ok(previous_entries) = entries(previous) {
            for failure in register(app, &previous_entries, &state, &mut registered) {
                tracing::warn!("{}", failure.message);
            }
        }
        let detail = blocking
            .iter()
            .map(|failure| failure.message.clone())
            .collect::<Vec<_>>()
            .join("；");
        return Err(format!("快捷键未生效：{detail}"));
    }
    for failure in failures {
        tracing::warn!("{}", failure.message);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalShortcut {
    pub action: UiAction,
    pub modifiers: InputModifiers,
    pub key_code: u32,
}

pub fn local_shortcuts(settings: &ShortcutSettings) -> Vec<LocalShortcut> {
    [
        (UiAction::ToggleMode, &settings.toggle_mode),
        (UiAction::ToggleStage, &settings.toggle_stage),
        (UiAction::ShowCalibration, &settings.open_calibration),
        (UiAction::ToggleRemotePause, &settings.pause_remote),
        (UiAction::ToggleEdge, &settings.toggle_edge),
    ]
    .into_iter()
    .filter_map(|(action, value)| {
        parse_local_shortcut(value).map(|(modifiers, key_code)| LocalShortcut {
            action,
            modifiers,
            key_code,
        })
    })
    .collect()
}

fn parse_local_shortcut(value: &str) -> Option<(InputModifiers, u32)> {
    let mut modifiers = InputModifiers::NONE;
    let mut key_code = None;
    for part in value
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers = modifiers.union(InputModifiers::CONTROL),
            "alt" => modifiers = modifiers.union(InputModifiers::ALT),
            "shift" => modifiers = modifiers.union(InputModifiers::SHIFT),
            "super" | "meta" | "win" | "cmd" | "command" => {
                modifiers = modifiers.union(InputModifiers::META)
            }
            _ => {
                if key_code.is_some() {
                    return None;
                }
                key_code = hid_key_code(part);
            }
        }
    }
    key_code.map(|key| (modifiers, key))
}

fn hid_key_code(key: &str) -> Option<u32> {
    if key.len() == 1 {
        let character = key.chars().next()?;
        if character.is_ascii_alphabetic() {
            return Some(0x04 + character.to_ascii_uppercase() as u32 - 'A' as u32);
        }
        if character.is_ascii_digit() {
            return Some(if character == '0' {
                0x27
            } else {
                0x1E + character as u32 - '1' as u32
            });
        }
    }
    match key {
        "Space" => Some(0x2C),
        "Enter" => Some(0x28),
        "Tab" => Some(0x2B),
        "Backspace" => Some(0x2A),
        "Delete" => Some(0x4C),
        "Home" => Some(0x4A),
        "End" => Some(0x4D),
        "PageUp" => Some(0x4B),
        "PageDown" => Some(0x4E),
        "Insert" => Some(0x49),
        "ArrowRight" => Some(0x4F),
        "ArrowLeft" => Some(0x50),
        "ArrowDown" => Some(0x51),
        "ArrowUp" => Some(0x52),
        _ => {
            let number: u32 = key.strip_prefix('F')?.parse().ok()?;
            match number {
                1..=12 => Some(0x3A + number - 1),
                13..=24 => Some(0x68 + number - 13),
                _ => None,
            }
        }
    }
}

