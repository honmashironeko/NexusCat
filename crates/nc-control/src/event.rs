use nc_core::DeviceInfo;
use nc_proto::{ControlMessage, LinkPurpose};
use std::fmt;

#[derive(Clone, PartialEq)]
pub enum ControlEvent {
    Connected {
        peer: DeviceInfo,
        trusted: bool,
        certificate_fingerprint: [u8; 32],
        media_key: [u8; 32],
        purpose: LinkPurpose,
    },
    PairingSas { peer: DeviceInfo, sas: String },
    PairingSucceeded { peer: DeviceInfo },
    PairingFailed { reason: String },
    Message(ControlMessage),
    Heartbeat { rtt_us: u64 },
    RemoteClock { offset_us: i64 },
    Disconnected { reason: String },
}

impl fmt::Debug for ControlEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connected {
                peer,
                trusted,
                certificate_fingerprint,
                media_key: _,
                purpose,
            } => formatter
                .debug_struct("Connected")
                .field("peer", peer)
                .field("trusted", trusted)
                .field("certificate_fingerprint", certificate_fingerprint)
                .field("media_key", &"<redacted>")
                .field("purpose", purpose)
                .finish(),
            Self::PairingSas { peer, sas } => formatter
                .debug_struct("PairingSas")
                .field("peer", peer)
                .field("sas", sas)
                .finish(),
            Self::PairingSucceeded { peer } => formatter
                .debug_struct("PairingSucceeded")
                .field("peer", peer)
                .finish(),
            Self::PairingFailed { reason } => formatter
                .debug_struct("PairingFailed")
                .field("reason", reason)
                .finish(),
            Self::Message(message) => formatter.debug_tuple("Message").field(message).finish(),
            Self::Heartbeat { rtt_us } => formatter
                .debug_struct("Heartbeat")
                .field("rtt_us", rtt_us)
                .finish(),
            Self::RemoteClock { offset_us } => formatter
                .debug_struct("RemoteClock")
                .field("offset_us", offset_us)
                .finish(),
            Self::Disconnected { reason } => formatter
                .debug_struct("Disconnected")
                .field("reason", reason)
                .finish(),
        }
    }
}
