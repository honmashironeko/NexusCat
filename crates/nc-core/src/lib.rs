pub mod device;
pub mod display;
pub mod error;
pub mod ids;
pub mod input;
pub mod media;
pub mod mode;
pub mod notification;
pub mod permission;
pub mod session;
pub mod settings;

pub use device::{Capabilities, DeviceInfo, Platform};
pub use display::{DisplayNode, PhysicalSize, Point, Rect, ScreenOrientation, Size};
pub use error::{ErrorKind, NexusError, Result};
pub use ids::{DeviceId, DisplayNodeId, FrameId, SessionId, StreamId};
pub use input::{
    InputEnvelope, InputEvent, KeyState, Modifiers, MouseButton, MouseButtonState,
};
pub use media::{
    AudioCodec, AudioFrame, EncodedAudioFrame, EncodedVideoFrame, PixelFormat, VideoFrame,
};
pub use mode::{Direction, InputOwner, Mode, UiState};
pub use notification::{
    AppNotification, NotificationAction, NotificationActionKind, NotificationCategory, Severity,
};
pub use permission::{PermissionKind, PermissionReport, PermissionStatus};
pub use session::{ConnectionPhase, SessionSnapshot, SessionState};
pub use settings::{
    AppSettings, Language, SharedConfig, SharedConfigSnapshot, ShortcutSettings, Theme, VideoTarget,
};
