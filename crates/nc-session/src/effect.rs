use nc_core::{DeviceId, Mode, UiState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEffect {
    StartDiscovery,
    StopDiscovery,
    Connect,
    StartPairing,
    Disconnect,
    StartMedia { target: Option<DeviceId> },
    StopMedia,
    ShowControlPanel,
    ShowStage,
    HideStage,
    BackToBackground,
    SwitchWindowState(UiState),
    ReleaseInput,
    GrantInput(DeviceId),
    PersistMode(Mode),
    Notice(SessionNotice),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionNotice {
    Paired,
    PairingFailed,
    Connected,
    Disconnected,
    Reconnecting,
    ReconnectFailed,
    ModeChanged,
    PermissionLost,
    EmergencyRelease,
    ControlHandover,
    ControlReturned,
}
