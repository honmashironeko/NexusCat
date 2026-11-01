use nc_core::{DeviceId, DeviceInfo, Direction, DisplayNodeId, InputOwner, Mode, UiState};
use nc_topology::Layout;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEvent {
    StartDiscovery,
    StopDiscovery,
    DevicesUpdated,
    ConnectRequested {
        device: DeviceId,
        trusted: bool,
    },
    PairingStarted {
        device: DeviceId,
    },
    SasRequired,
    PairingSucceeded {
        device: DeviceId,
    },
    PairingFailed {
        reason: String,
    },
    ConnectionEstablished {
        peer: DeviceInfo,
    },
    SwitchTarget {
        peer: DeviceInfo,
    },
    RemoteSessionClosed,
    ConnectionLost {
        reason: String,
    },
    ReconnectTick {
        attempt: u32,
        max: u32,
    },
    SetMode {
        mode: Mode,
    },
    ToggleMode,
    EdgeCrossed {
        to_device: DeviceId,
        to_node: DisplayNodeId,
        direction: Direction,
    },
    ReturnToLocal,
    LocalDisplayNodeChanged {
        node: DisplayNodeId,
    },
    RemoteDisplayNodeChanged {
        node: DisplayNodeId,
    },
    ReleaseAll,
    StageSummon,
    StageHide,
    UiStateChanged {
        ui_state: UiState,
    },
    PermissionLost,
    PermissionRestored,
    PeerInputOwner(InputOwner),
    UserAbort,
    EmergencyRelease,
    Shutdown,
    LayoutUpdated {
        layout: Layout,
    },
}
