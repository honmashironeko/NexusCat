use bincode::Options;
use nc_core::DeviceInfo;
use serde::{Deserialize, Serialize};

pub const ANNOUNCEMENT_MAGIC: u32 = 0x4E43_5331;

pub const ANNOUNCEMENT_VERSION: u16 = 1;

pub const MAX_ANNOUNCEMENT_BYTES: usize = 1200;

const HEADER_BYTES: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announcement {
    pub magic: u32,
    pub version: u16,
    pub device: DeviceInfo,
    pub sent_at_ms: u64,
}

impl Announcement {
    pub fn new(device: DeviceInfo, sent_at_ms: u64) -> Self {
        Self {
            magic: ANNOUNCEMENT_MAGIC,
            version: ANNOUNCEMENT_VERSION,
            device,
            sent_at_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnnouncementError {
    #[error("通告载荷过大：{size} 字节，必须小于 {limit} 字节")]
    TooLarge { size: usize, limit: usize },

    #[error("通告 magic 不匹配：0x{found:08x}")]
    InvalidMagic { found: u32 },

    #[error("通告版本不兼容：对端 {found}，本机 {expected}")]
    VersionMismatch { found: u16, expected: u16 },

    #[error("通告载荷截断：至少需要 {needed} 字节，实际 {actual} 字节")]
    Truncated { needed: usize, actual: usize },

    #[error("通告编解码失败：{0}")]
    Codec(String),
}

pub fn encode_announcement(announcement: &Announcement) -> Result<Vec<u8>, AnnouncementError> {
    if announcement.magic != ANNOUNCEMENT_MAGIC {
        return Err(AnnouncementError::InvalidMagic {
            found: announcement.magic,
        });
    }
    if announcement.version != ANNOUNCEMENT_VERSION {
        return Err(AnnouncementError::VersionMismatch {
            found: announcement.version,
            expected: ANNOUNCEMENT_VERSION,
        });
    }

    let payload = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
        .serialize(announcement)
        .map_err(|error| AnnouncementError::Codec(error.to_string()))?;
    if payload.len() >= MAX_ANNOUNCEMENT_BYTES {
        return Err(AnnouncementError::TooLarge {
            size: payload.len(),
            limit: MAX_ANNOUNCEMENT_BYTES,
        });
    }
    Ok(payload)
}

pub fn decode_announcement(payload: &[u8]) -> Result<Announcement, AnnouncementError> {
    if payload.len() >= MAX_ANNOUNCEMENT_BYTES {
        return Err(AnnouncementError::TooLarge {
            size: payload.len(),
            limit: MAX_ANNOUNCEMENT_BYTES,
        });
    }
    if payload.len() < HEADER_BYTES {
        return Err(AnnouncementError::Truncated {
            needed: HEADER_BYTES,
            actual: payload.len(),
        });
    }

    let magic = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    if magic != ANNOUNCEMENT_MAGIC {
        return Err(AnnouncementError::InvalidMagic { found: magic });
    }
    let version = u16::from_le_bytes([payload[4], payload[5]]);
    if version != ANNOUNCEMENT_VERSION {
        return Err(AnnouncementError::VersionMismatch {
            found: version,
            expected: ANNOUNCEMENT_VERSION,
        });
    }

    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(MAX_ANNOUNCEMENT_BYTES as u64)
        .reject_trailing_bytes()
        .deserialize(payload)
        .map_err(|error| AnnouncementError::Codec(error.to_string()))
}
