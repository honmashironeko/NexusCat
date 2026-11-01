use crate::runtime::{EncoderClass, VideoCapabilities};
use nc_core::{
    AppNotification, AppSettings, DeviceId, DeviceInfo, DisplayNode, Language, NexusError,
    PermissionReport, Platform, SessionSnapshot,
};
use nc_observability::MetricsSnapshot;
use nc_proto::PackageKind;
use nc_topology::Layout;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

pub const EVENT_SESSION_CHANGED: &str = "session-changed";
pub const EVENT_DEVICES_CHANGED: &str = "devices-changed";
pub const EVENT_LAYOUT_CHANGED: &str = "layout-changed";
pub const EVENT_SETTINGS_CHANGED: &str = "settings-changed";
pub const EVENT_SHARED_SYNC_STATUS: &str = "shared-sync-status";
pub const EVENT_GROUP_STATUS: &str = "group-status";
pub const EVENT_NOTIFICATION: &str = "notification";
pub const EVENT_NOTIFICATIONS_CHANGED: &str = "notifications-changed";
pub const EVENT_PAIRING: &str = "pairing-event";
pub const EVENT_METRICS: &str = "metrics";
pub const EVENT_TRANSFER_PROGRESS: &str = "transfer-progress";
pub const EVENT_TRANSFERS_CHANGED: &str = "transfers-changed";
pub const EVENT_UPDATE_STATUS: &str = "update-status";
pub const EVENT_PERMISSION_CHANGED: &str = "permission-changed";
pub const EVENT_MEDIA_RESTART: &str = "media-restart";
pub const EVENT_STAGE_COMMAND: &str = "stage-command";
pub const EVENT_STAGE_SWITCHING: &str = "stage-switching";
pub const EVENT_CALIBRATION_OPEN: &str = "calibration-open";
pub const EVENT_OPEN_WORKBENCH: &str = "open-workbench";
pub const EVENT_OPEN_SETTINGS: &str = "open-settings";
pub const EVENT_OPEN_UPDATE: &str = "open-update";
pub const EVENT_REMOTE_CURSOR: &str = "remote-cursor";
pub const EVENT_REMOTE_PAUSE: &str = "remote-pause";
pub const EVENT_STAGE_EDGE_HINT: &str = "stage-edge-hint";
pub const EVENT_EDGE_OSD: &str = "edge-osd";
pub const EVENT_EDGE_HALO: &str = "edge-halo";
pub const EVENT_CONTROL_OSD: &str = "control-osd";
pub const EVENT_FILE_DROP_OSD: &str = "file-drop-osd";
pub const EVENT_FILE_DROP_OSD_HIDDEN: &str = "file-drop-osd-hidden";
pub const EVENT_FILE_DROP_HOVER: &str = "file-drop-hover";
pub const EVENT_TRAY_MODE_TOGGLE: &str = "tray-mode-toggle";
pub const EVENT_REQUEST_QUIT: &str = "request-quit";

#[derive(Debug, Clone, Serialize)]
pub struct EdgeOsdView {
    pub direction: String,
    pub kicker: String,
    pub title: String,
    pub display: String,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EdgeHaloView {
    pub direction: String,
    pub title: String,
    pub display: String,
    pub hint: String,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ControlOsdView {
    pub kicker: String,
    pub title: String,
    pub subtitle: String,
    pub hint: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileDropCardView {
    pub node_id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileDropCardsView {
    pub scale: f64,
    pub cards: Vec<FileDropCardView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDropOsdView {
    pub nodes: Vec<DisplayNode>,
    pub file_count: Option<usize>,
    pub enabled: bool,
    pub language: Language,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDropHoverView {
    pub node_id: Option<String>,
    pub file_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputServiceStatusView {
    pub installed: bool,
    pub running: bool,
    pub connected: bool,
    pub version: Option<String>,
    pub needs_update: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteCursorView {
    pub device_id: DeviceId,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemotePauseView {
    pub paused: bool,
    pub shortcut: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageEdgeHintView {
    pub device: String,
    pub display: String,
    pub direction: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl CommandError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }
}

impl From<NexusError> for CommandError {
    fn from(error: NexusError) -> Self {
        use nc_core::ErrorKind;
        let code = match error.kind() {
            ErrorKind::InvalidInput => "invalid_input",
            ErrorKind::Business => "business",
            ErrorKind::Infrastructure => "infrastructure",
            ErrorKind::Fatal => "fatal",
        };
        Self::new(code, error.to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaBridgeInfo {
    pub port: u16,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct WebCodecsProbe {
    pub supported: bool,
    pub h264: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceView {
    pub info: DeviceInfo,
    pub online: bool,
    pub connected: bool,
    pub trusted: bool,
    pub address: Option<String>,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct GroupStatusView {
    pub active_host: Option<DeviceId>,
    pub pending_target: Option<DeviceId>,
    pub takeover_target: Option<DeviceId>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SharedConfigStatusView {
    pub state: &'static str,
    pub revision: u64,
    pub synced_devices: usize,
    pub online_devices: usize,
    pub total_devices: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoCapabilitiesView {
    pub class: &'static str,
    pub hardware_encode: bool,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
}

impl VideoCapabilitiesView {
    pub fn from_runtime(capabilities: &VideoCapabilities) -> Self {
        let (max_width, max_height, max_fps) = capabilities.limits();
        let class = match capabilities.class {
            EncoderClass::Software => "software",
            EncoderClass::Integrated => "integrated",
            EncoderClass::Discrete => "discrete",
        };
        Self {
            class,
            hardware_encode: capabilities.hardware_encode,
            max_width,
            max_height,
            max_fps,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Bootstrap {
    pub version: String,
    pub platform: Platform,
    pub data_dir: String,
    pub settings: AppSettings,
    pub session: SessionSnapshot,
    pub devices: Vec<DeviceView>,
    pub layout: Layout,
    pub notifications: Vec<AppNotification>,
    pub transfers: Vec<TransferView>,
    pub permissions: PermissionReport,
    pub metrics: MetricsSnapshot,
    pub media_bridge: MediaBridgeInfo,
    pub webcodecs: WebCodecsProbe,
    pub video_capabilities: VideoCapabilitiesView,
    pub shared_config: SharedConfigStatusView,
    pub group_status: GroupStatusView,
    pub test_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Send,
    Receive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Offered,
    Transferring,
    Verifying,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferView {
    pub id: u64,
    pub direction: TransferDirection,
    pub file_name: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub state: TransferState,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Available,
    Receiving,
    Staged,
    Applying,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePeerState {
    Queued,
    Connecting,
    Transferring,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdatePeerView {
    pub device_id: DeviceId,
    pub device_name: String,
    pub state: UpdatePeerState,
    pub progress_percent: u8,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatusView {
    pub state: UpdateState,
    pub version: Option<String>,
    pub kind: Option<PackageKind>,
    pub progress_percent: u8,
    pub message: Option<String>,
    pub rollback_versions: Vec<String>,
    pub peers: Vec<UpdatePeerView>,
}

impl Default for UpdateStatusView {
    fn default() -> Self {
        Self {
            state: UpdateState::Idle,
            version: None,
            kind: None,
            progress_percent: 0,
            message: None,
            rollback_versions: Vec::new(),
            peers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingStage {
    SasRequired,
    Success,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairingEventPayload {
    pub stage: PairingStage,
    pub device_id: Option<DeviceId>,
    pub device_name: Option<String>,
    pub sas: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageCommand {
    pub action: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageSwitchingView {
    pub active: bool,
}

pub fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if let Err(error) = app.emit(event, payload) {
        tracing::debug!(%error, event, "发送前端事件失败");
    }
}
