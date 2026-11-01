use nc_core::DeviceId;

pub const PROBE_MAGIC: u32 = 0x4E43_5031;

pub const PROBE_VERSION: u16 = 1;

pub const PROBE_REQUEST_BYTES: usize = 22;

const HEADER_BYTES: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProbeRequestError {
    #[error("探测请求 magic 不匹配：0x{found:08x}")]
    InvalidMagic { found: u32 },

    #[error("探测请求版本不兼容：对端 {found}，本机 {expected}")]
    VersionMismatch { found: u16, expected: u16 },

    #[error("探测请求长度应为 {expected} 字节，实际 {actual} 字节")]
    InvalidLength { expected: usize, actual: usize },
}

pub fn has_probe_magic(payload: &[u8]) -> bool {
    payload.len() >= HEADER_BYTES
        && u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) == PROBE_MAGIC
}

pub fn encode_probe_request(device: DeviceId) -> [u8; PROBE_REQUEST_BYTES] {
    let mut payload = [0u8; PROBE_REQUEST_BYTES];
    payload[..4].copy_from_slice(&PROBE_MAGIC.to_le_bytes());
    payload[4..6].copy_from_slice(&PROBE_VERSION.to_le_bytes());
    payload[6..].copy_from_slice(device.as_bytes());
    payload
}

pub fn decode_probe_request(payload: &[u8]) -> Result<DeviceId, ProbeRequestError> {
    if payload.len() != PROBE_REQUEST_BYTES {
        return Err(ProbeRequestError::InvalidLength {
            expected: PROBE_REQUEST_BYTES,
            actual: payload.len(),
        });
    }

    let magic = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    if magic != PROBE_MAGIC {
        return Err(ProbeRequestError::InvalidMagic { found: magic });
    }
    let version = u16::from_le_bytes([payload[4], payload[5]]);
    if version != PROBE_VERSION {
        return Err(ProbeRequestError::VersionMismatch {
            found: version,
            expected: PROBE_VERSION,
        });
    }

    let mut device = [0u8; 16];
    device.copy_from_slice(&payload[6..]);
    Ok(DeviceId::from_bytes(device))
}

