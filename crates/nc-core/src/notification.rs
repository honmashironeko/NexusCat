use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationCategory {
    Connection,
    Pairing,
    Permission,
    Update,
    FileTransfer,
    Media,
    System,
}

impl NotificationCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connection => "connection",
            Self::Pairing => "pairing",
            Self::Permission => "permission",
            Self::Update => "update",
            Self::FileTransfer => "file_transfer",
            Self::Media => "media",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationActionKind {
    OpenControlPanel,
    OpenSettings,
    ShowStage,
    RetryConnection,
    OpenUpdate,
    Dismiss,
}

impl NotificationActionKind {
    pub const fn action_id(self) -> &'static str {
        match self {
            Self::OpenControlPanel => "open_control_panel",
            Self::OpenSettings => "open_settings",
            Self::ShowStage => "show_stage",
            Self::RetryConnection => "retry_connection",
            Self::OpenUpdate => "open_update",
            Self::Dismiss => "dismiss",
        }
    }

    pub fn from_action_id(action_id: &str) -> Option<Self> {
        match action_id {
            "tap" | "open_control_panel" => Some(Self::OpenControlPanel),
            "open_settings" => Some(Self::OpenSettings),
            "show_stage" => Some(Self::ShowStage),
            "retry_connection" => Some(Self::RetryConnection),
            "open_update" => Some(Self::OpenUpdate),
            "dismiss" => Some(Self::Dismiss),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationAction {
    pub kind: NotificationActionKind,
    pub label: String,
    pub payload: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppNotification {
    pub id: u64,
    pub severity: Severity,
    pub category: NotificationCategory,
    pub title: String,
    pub body: String,
    pub action: Option<NotificationAction>,
    pub timestamp_ms: u64,
    pub read: bool,
}

impl AppNotification {
    pub fn is_actionable(&self) -> bool {
        matches!(self.severity, Severity::Warning | Severity::Error) || self.action.is_some()
    }
}

