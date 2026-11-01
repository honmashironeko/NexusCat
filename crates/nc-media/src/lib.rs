pub mod config;
pub mod fragment;
pub mod key;
pub mod reassembly;
pub mod receiver;
pub mod sender;
pub mod stats;
pub mod wire;

pub use nc_core::{EncodedVideoFrame, StreamId};

pub use config::{
    MediaConfig, DEFAULT_FRAME_TIMEOUT_MS, DEFAULT_MAX_PENDING_FRAMES, DEFAULT_MTU,
    DEFAULT_NACK_DELAY_MS, DEFAULT_REORDER_WINDOW_MS, MAX_FEC_PARITY,
};
pub use fragment::Fragmenter;
pub use key::MediaKey;
pub use reassembly::{AssembledFrame, NackRequest, Reassembler, ReassemblyEvent};
pub use receiver::{MediaEvent, MediaReceiver};
pub use sender::MediaSender;
pub use stats::MediaStats;
pub use wire::{
    MediaHeader, MediaPacket, NackBody, PacketKind, AAD_LEN, FLAG_AUDIO, FLAG_FEC_PARITY,
    FLAG_KEYFRAME, HEADER_LEN, MAGIC, MAX_DATAGRAM_BYTES, MAX_NACK_BODY_BYTES, MAX_NACK_MISSING,
    TAG_LEN, WIRE_VERSION,
};
