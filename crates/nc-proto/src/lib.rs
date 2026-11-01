pub mod error;
pub mod framing;
pub mod message;

pub use error::ProtocolError;
pub use framing::{
    decode_frame, encode_frame, ControlFrame, FrameKind, MAX_CONTROL_FRAME_BYTES,
    MAX_HANDSHAKE_FRAME_BYTES,
};
pub use message::{
    decode_control, decode_handshake, encode_control, encode_handshake, ClipboardContent,
    ControlMessage, FileTransferId, HandshakeMessage, Hello, HelloAck, LinkPurpose, PackageKind,
    StreamRequest, UpdateManifest, HANDSHAKE_FEATURES,
};
