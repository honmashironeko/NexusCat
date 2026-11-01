use nc_core::{DeviceId, Mode, NotificationActionKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    ShowControlPanel,
    ShowCalibration,
    ShowSettings,
    HideToBackground,
    ToggleEdge,
    SummonStage,
    HideStage,
    ToggleStage,
    ToggleRemotePause,
    SetMode(Mode),
    ToggleMode,
    StartDiscovery,
    StopDiscovery,
    PairDevice(DeviceId),
    ConnectDevice(DeviceId),
    Disconnect,
    OpenNotifications,
    MarkAllNotificationsRead,
    NotificationAction {
        kind: NotificationActionKind,
        payload: Option<String>,
    },
    Quit,
}
