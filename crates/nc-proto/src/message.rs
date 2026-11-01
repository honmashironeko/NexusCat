use nc_core::{
    AppNotification, AudioCodec, DeviceId, DeviceInfo, Direction, DisplayNode, InputEnvelope, Mode,
    PermissionReport, Platform, SessionId, SharedConfigSnapshot, StreamId, VideoTarget,
};
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct FileTransferId(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandshakeMessage {
    Hello(Hello),
    HelloAck(HelloAck),
    PairSas { sas: String },
    PairConfirm { sas: String },
    PairResult {
        accepted: bool,
        reason: Option<String>,
    },
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkPurpose {
    Group,
    Update,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub device: DeviceInfo,
    pub static_public: [u8; 32],
    pub ephemeral_public: [u8; 32],
    pub random: [u8; 32],
    pub wants_pairing: bool,
    pub purpose: LinkPurpose,
    pub features: u32,
}

pub const HANDSHAKE_FEATURES: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloAck {
    pub device: DeviceInfo,
    pub static_public: [u8; 32],
    pub ephemeral_public: [u8; 32],
    pub random: [u8; 32],
    pub trusted: bool,
    pub pairing_required: bool,
    pub features: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardContent {
    Text(String),
    ImagePng(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    Portable,
    Installer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub platform: Platform,
    pub kind: PackageKind,
    pub size: u64,
    pub sha256: [u8; 32],
    pub notes: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamRequest {
    pub stream_id: StreamId,
    pub target: VideoTarget,
    pub audio: bool,
    pub monitor_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMessage {
    Heartbeat {
        seq: u64,
        timestamp_us: u64,
    },
    HeartbeatAck {
        seq: u64,
        timestamp_us: u64,
    },
    SessionOpen {
        mode: Mode,
        target: VideoTarget,
    },
    SessionAccept {
        session_id: SessionId,
    },
    SessionReject {
        reason: String,
    },
    SessionClose,
    HostStatus {
        host: DeviceId,
        active: bool,
    },
    ModeChange {
        mode: Mode,
    },
    Input {
        envelope: InputEnvelope,
    },
    ReleaseAll,
    EdgeReturn {
        direction: Direction,
    },
    Clipboard {
        content: ClipboardContent,
    },
    FileOffer {
        transfer_id: FileTransferId,
        file_name: String,
        size: u64,
        sha256: [u8; 32],
    },
    FileAccept {
        transfer_id: FileTransferId,
        offset: u64,
    },
    FileReject {
        transfer_id: FileTransferId,
        reason: String,
    },
    FileChunk {
        transfer_id: FileTransferId,
        offset: u64,
        data: Vec<u8>,
    },
    FileComplete {
        transfer_id: FileTransferId,
    },
    FileResult {
        transfer_id: FileTransferId,
        success: bool,
        message: String,
    },
    UpdateManifest {
        manifest: UpdateManifest,
    },
    UpdateDecision {
        version: String,
        accepted: bool,
    },
    UpdateChunk {
        version: String,
        offset: u64,
        data: Vec<u8>,
    },
    UpdateComplete {
        version: String,
    },
    UpdateResult {
        version: String,
        success: bool,
        message: String,
    },
    TopologySync {
        nodes: Vec<DisplayNode>,
    },
    StreamRequest {
        request: StreamRequest,
    },
    StreamInfo {
        stream_id: StreamId,
        device_id: DeviceId,
        width: u32,
        height: u32,
        fps: u32,
        monitor_index: Option<u32>,
    },
    AudioStreamInfo {
        stream_id: StreamId,
        device_id: DeviceId,
        codec: AudioCodec,
        sample_rate: u32,
        channels: u16,
    },
    StreamStop {
        stream_id: StreamId,
    },
    KeyframeRequest {
        stream_id: StreamId,
    },
    PermissionSync {
        report: PermissionReport,
    },
    Notify {
        notification: AppNotification,
    },
    Ack {
        seq: u64,
    },
    Goodbye {
        reason: String,
    },
    Error {
        code: u32,
        message: String,
    },
    SharedConfig {
        snapshot: SharedConfigSnapshot,
    },
    SharedConfigAck {
        revision: u64,
    },
    HostReleaseRequest {
        requester: DeviceId,
    },
}

impl ControlMessage {
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Heartbeat { .. } => "heartbeat",
            Self::HeartbeatAck { .. } => "heartbeat_ack",
            Self::SessionOpen { .. } => "session_open",
            Self::SessionAccept { .. } => "session_accept",
            Self::SessionReject { .. } => "session_reject",
            Self::SessionClose => "session_close",
            Self::HostStatus { .. } => "host_status",
            Self::ModeChange { .. } => "mode_change",
            Self::Input { .. } => "input",
            Self::ReleaseAll => "release_all",
            Self::EdgeReturn { .. } => "edge_return",
            Self::Clipboard { .. } => "clipboard",
            Self::FileOffer { .. } => "file_offer",
            Self::FileAccept { .. } => "file_accept",
            Self::FileReject { .. } => "file_reject",
            Self::FileChunk { .. } => "file_chunk",
            Self::FileComplete { .. } => "file_complete",
            Self::FileResult { .. } => "file_result",
            Self::UpdateManifest { .. } => "update_manifest",
            Self::UpdateDecision { .. } => "update_decision",
            Self::UpdateChunk { .. } => "update_chunk",
            Self::UpdateComplete { .. } => "update_complete",
            Self::UpdateResult { .. } => "update_result",
            Self::TopologySync { .. } => "topology_sync",
            Self::StreamRequest { .. } => "stream_request",
            Self::StreamInfo { .. } => "stream_info",
            Self::AudioStreamInfo { .. } => "audio_stream_info",
            Self::StreamStop { .. } => "stream_stop",
            Self::KeyframeRequest { .. } => "keyframe_request",
            Self::PermissionSync { .. } => "permission_sync",
            Self::Notify { .. } => "notify",
            Self::Ack { .. } => "ack",
            Self::Goodbye { .. } => "goodbye",
            Self::Error { .. } => "error",
            Self::SharedConfig { .. } => "shared_config",
            Self::SharedConfigAck { .. } => "shared_config_ack",
            Self::HostReleaseRequest { .. } => "host_release_request",
        }
    }

    pub const fn is_high_priority(&self) -> bool {
        matches!(
            self,
            Self::Input { .. }
                | Self::ReleaseAll
                | Self::EdgeReturn { .. }
                | Self::Heartbeat { .. }
                | Self::HostReleaseRequest { .. }
                | Self::KeyframeRequest { .. }
                | Self::ModeChange { .. }
        )
    }
}

fn codec_options() -> impl bincode::Options {
    use bincode::Options;
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
}

pub fn encode_handshake(message: &HandshakeMessage) -> Result<Vec<u8>, crate::ProtocolError> {
    use bincode::Options;
    codec_options()
        .serialize(message)
        .map_err(|error| crate::ProtocolError::Serialize(error.to_string()))
}

pub fn decode_handshake(bytes: &[u8]) -> Result<HandshakeMessage, crate::ProtocolError> {
    use bincode::Options;
    codec_options()
        .deserialize(bytes)
        .map_err(|error| crate::ProtocolError::Deserialize(error.to_string()))
}

pub fn encode_control(message: &ControlMessage) -> Result<Vec<u8>, crate::ProtocolError> {
    use bincode::Options;
    codec_options()
        .serialize(message)
        .map_err(|error| crate::ProtocolError::Serialize(error.to_string()))
}

pub fn decode_control(bytes: &[u8]) -> Result<ControlMessage, crate::ProtocolError> {
    use bincode::Options;
    codec_options()
        .deserialize(bytes)
        .map_err(|error| crate::ProtocolError::Deserialize(error.to_string()))
}

