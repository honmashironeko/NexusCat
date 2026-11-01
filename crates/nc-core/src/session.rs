use crate::device::DeviceInfo;
use crate::ids::{DeviceId, DisplayNodeId};
use crate::mode::{InputOwner, Mode, UiState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    #[default]
    Idle,
    Discovering,
    Pairing,
    Connected,
    Kvm,
    Aggregating,
    Reconnecting,
    Error,
    Closed,
}

impl SessionState {
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Kvm | Self::Aggregating)
    }

    pub const fn is_connected(self) -> bool {
        matches!(
            self,
            Self::Connected | Self::Kvm | Self::Aggregating | Self::Reconnecting
        )
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Discovering => "discovering",
            Self::Pairing => "pairing",
            Self::Connected => "connected",
            Self::Kvm => "kvm",
            Self::Aggregating => "aggregating",
            Self::Reconnecting => "reconnecting",
            Self::Error => "error",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionPhase {
    #[default]
    Offline,
    Discovering,
    TlsHandshake,
    Pairing,
    Authenticating,
    Ready,
    Reconnecting,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub state: SessionState,
    pub phase: ConnectionPhase,
    pub mode: Mode,
    pub ui_state: UiState,
    pub peer: Option<DeviceInfo>,
    pub input_owner: InputOwner,
    pub stage_visible: bool,
    pub connected_since_ms: Option<u64>,
    pub last_error: Option<String>,
    pub current_display_device: Option<DeviceId>,
    pub current_display_node: Option<DisplayNodeId>,
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            state: SessionState::Idle,
            phase: ConnectionPhase::Offline,
            mode: Mode::Kvm,
            ui_state: UiState::Background,
            peer: None,
            input_owner: InputOwner::Local,
            stage_visible: false,
            connected_since_ms: None,
            last_error: None,
            current_display_device: None,
            current_display_node: None,
        }
    }
}

impl SessionSnapshot {
    pub fn media_active(&self) -> bool {
        self.state == SessionState::Aggregating && self.stage_visible
    }
}

