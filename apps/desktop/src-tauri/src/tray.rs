use crate::actions::UiAction;
use nc_core::{AppSettings, Language, Mode, SessionSnapshot, SessionState};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Wry};
use tokio::sync::mpsc::UnboundedSender;

pub struct TrayHandle {
    _tray: TrayIcon<Wry>,
    status_item: MenuItem<Wry>,
    open_item: MenuItem<Wry>,
    settings_item: MenuItem<Wry>,
    mode_menu: Submenu<Wry>,
    kvm_item: CheckMenuItem<Wry>,
    aggregation_item: CheckMenuItem<Wry>,
    edge_item: CheckMenuItem<Wry>,
    stage_item: MenuItem<Wry>,
    quit_item: MenuItem<Wry>,
}

impl TrayHandle {
    pub fn update(
        &self,
        snapshot: &SessionSnapshot,
        unread: usize,
        language: Language,
        settings: &AppSettings,
    ) {
        let status = status_text(snapshot, unread, language);
        let _ = self.status_item.set_text(&status);
        let _ = self._tray.set_tooltip(Some(&status));

        let _ = self
            .open_item
            .set_text(text(language, "打开工作台", "Open Workbench"));
        let _ = self
            .settings_item
            .set_text(text(language, "设置…", "Settings…"));
        let _ = self.mode_menu.set_text(text(language, "模式", "Mode"));
        let _ = self.kvm_item.set_text(text(
            language,
            "多屏流转（软 KVM）",
            "Multi-screen flow (KVM)",
        ));
        let _ = self.aggregation_item.set_text(text(
            language,
            "单屏聚合（全屏）",
            "Single-screen aggregation",
        ));
        let _ = self.kvm_item.set_checked(snapshot.mode == Mode::Kvm);
        let _ = self
            .aggregation_item
            .set_checked(snapshot.mode == Mode::Aggregation);
        let _ = self
            .edge_item
            .set_text(text(language, "边缘触发", "Edge takeover"));
        let _ = self.edge_item.set_checked(settings.edge_enabled);
        let stage_label = if snapshot.stage_visible {
            text(language, "退出全屏", "Exit fullscreen")
        } else {
            text(language, "进入全屏", "Enter fullscreen")
        };
        let _ = self.stage_item.set_text(stage_label);
        let _ = self
            .quit_item
            .set_text(text(language, "退出 NexusCat", "Quit NexusCat"));

        let connected = snapshot.state.is_connected();
        let _ = self.kvm_item.set_enabled(true);
        let _ = self.aggregation_item.set_enabled(true);
        let _ = self
            .stage_item
            .set_enabled(connected && snapshot.mode == Mode::Aggregation);
    }
}

pub fn create_tray(
    app: &AppHandle,
    actions: UnboundedSender<UiAction>,
) -> tauri::Result<TrayHandle> {
    let status_item = MenuItem::with_id(app, "status", "NexusCat · 后台", false, None::<&str>)?;
    let open_item = MenuItem::with_id(app, "open", "打开工作台", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let kvm_item = CheckMenuItem::with_id(
        app,
        "mode_kvm",
        "多屏流转（软 KVM）",
        true,
        true,
        None::<&str>,
    )?;
    let aggregation_item = CheckMenuItem::with_id(
        app,
        "mode_aggregation",
        "单屏聚合（全屏）",
        true,
        false,
        None::<&str>,
    )?;
    let mode_menu = Submenu::with_items(app, "模式", true, &[&kvm_item, &aggregation_item])?;
    let edge_item = CheckMenuItem::with_id(app, "edge", "边缘触发", true, true, None::<&str>)?;
    let stage_item = MenuItem::with_id(app, "stage", "进入全屏", false, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出 NexusCat", true, None::<&str>)?;
    let separator_one = PredefinedMenuItem::separator(app)?;
    let separator_two = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &separator_one,
            &open_item,
            &settings_item,
            &mode_menu,
            &edge_item,
            &stage_item,
            &separator_two,
            &quit_item,
        ],
    )?;

    let menu_actions = actions.clone();
    let mut builder = TrayIconBuilder::with_id("nexuscat-tray")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("NexusCat")
        .on_menu_event(move |_app, event| {
            let id = event.id().as_ref();
            let action = match id {
                "settings" => Some(UiAction::ShowSettings),
                "open" => Some(UiAction::ShowControlPanel),
                "mode_kvm" => Some(UiAction::SetMode(Mode::Kvm)),
                "mode_aggregation" => Some(UiAction::SetMode(Mode::Aggregation)),
                "edge" => Some(UiAction::ToggleEdge),
                "stage" => Some(UiAction::ToggleStage),
                "quit" => Some(UiAction::Quit),
                _ => None,
            };
            if let Some(action) = action {
                let _ = menu_actions.send(action);
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    let tray_actions = actions;
    builder = builder.on_tray_icon_event(move |_tray, event| {
        if matches!(
            event,
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        ) {
            let _ = tray_actions.send(UiAction::ShowControlPanel);
        }
    });
    let tray = builder.build(app)?;

    Ok(TrayHandle {
        _tray: tray,
        status_item,
        open_item,
        settings_item,
        mode_menu,
        kvm_item,
        aggregation_item,
        edge_item,
        stage_item,
        quit_item,
    })
}

fn text(language: Language, zh: &'static str, en: &'static str) -> &'static str {
    match language {
        Language::Zh => zh,
        Language::En => en,
    }
}

fn status_text(snapshot: &SessionSnapshot, unread: usize, language: Language) -> String {
    let state = match (language, snapshot.state) {
        (Language::Zh, SessionState::Idle) => "空闲",
        (Language::Zh, SessionState::Discovering) => "发现中",
        (Language::Zh, SessionState::Pairing) => "配对中",
        (Language::Zh, SessionState::Connected) => "已连接",
        (Language::Zh, SessionState::Kvm) => "多屏流转中",
        (Language::Zh, SessionState::Aggregating) => "单屏聚合待命",
        (Language::Zh, SessionState::Reconnecting) => "重连中",
        (Language::Zh, SessionState::Error) => "错误",
        (Language::Zh, SessionState::Closed) => "已关闭",
        (Language::En, SessionState::Idle) => "Idle",
        (Language::En, SessionState::Discovering) => "Discovering",
        (Language::En, SessionState::Pairing) => "Pairing",
        (Language::En, SessionState::Connected) => "Connected",
        (Language::En, SessionState::Kvm) => "Multi-screen flow",
        (Language::En, SessionState::Aggregating) => "Aggregation ready",
        (Language::En, SessionState::Reconnecting) => "Reconnecting",
        (Language::En, SessionState::Error) => "Error",
        (Language::En, SessionState::Closed) => "Closed",
    };
    let mut text = format!("NexusCat · {state}");
    if let Some(peer) = &snapshot.peer {
        text.push_str(&format!(" · {}", peer.name));
    }
    if unread > 0 {
        text.push_str(&match language {
            Language::Zh => format!(" · {unread} 未读"),
            Language::En => format!(" · {unread} unread"),
        });
    }
    text
}
