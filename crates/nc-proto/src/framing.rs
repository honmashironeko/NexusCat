use crate::error::ProtocolError;
use std::fmt;

pub const MAX_CONTROL_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_HANDSHAKE_FRAME_BYTES: usize = 64 * 1024;

const HEADER_LEN: usize = 4 + 1 + 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Handshake,
    Encrypted,
}

impl FrameKind {
    const fn to_byte(self) -> u8 {
        match self {
            Self::Handshake => 0,
            Self::Encrypted => 1,
        }
    }

    fn from_byte(byte: u8) -> Result<Self, ProtocolError> {
        match byte {
            0 => Ok(Self::Handshake),
            1 => Ok(Self::Encrypted),
            other => Err(ProtocolError::UnknownFrameKind(other)),
        }
    }
}

impl fmt::Display for FrameKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handshake => formatter.write_str("handshake"),
            Self::Encrypted => formatter.write_str("encrypted"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFrame {
    pub kind: FrameKind,
    pub seq: u64,
    pub body: Vec<u8>,
}

impl ControlFrame {
    pub fn handshake(body: Vec<u8>) -> Self {
        Self {
            kind: FrameKind::Handshake,
            seq: 0,
            body,
        }
    }

    pub fn encrypted(seq: u64, body: Vec<u8>) -> Self {
        Self {
            kind: FrameKind::Encrypted,
            seq,
            body,
        }
    }

    pub fn limit(&self) -> usize {
        match self.kind {
            FrameKind::Handshake => MAX_HANDSHAKE_FRAME_BYTES,
            FrameKind::Encrypted => MAX_CONTROL_FRAME_BYTES,
        }
    }
}

pub fn encode_frame(frame: &ControlFrame) -> Result<Vec<u8>, ProtocolError> {
    let total = HEADER_LEN + frame.body.len();
    if total > frame.limit() {
        return Err(ProtocolError::FrameTooLarge {
            actual: total,
            limit: frame.limit(),
        });
    }
    let mut buffer = Vec::with_capacity(total);
    buffer.extend_from_slice(&(frame.body.len() as u32).to_be_bytes());
    buffer.push(frame.kind.to_byte());
    buffer.extend_from_slice(&frame.seq.to_be_bytes());
    buffer.extend_from_slice(&frame.body);
    Ok(buffer)
}

pub fn decode_frame(bytes: &[u8]) -> Result<ControlFrame, ProtocolError> {
    if bytes.len() < HEADER_LEN {
        return Err(ProtocolError::Truncated {
            needed: HEADER_LEN,
            actual: bytes.len(),
        });
    }
    let body_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let kind = FrameKind::from_byte(bytes[4])?;
    let mut seq_bytes = [0u8; 8];
    seq_bytes.copy_from_slice(&bytes[5..13]);
    let seq = u64::from_be_bytes(seq_bytes);
    let expected_total = HEADER_LEN + body_len;
    if bytes.len() < expected_total {
        return Err(ProtocolError::Truncated {
            needed: expected_total,
            actual: bytes.len(),
        });
    }
    if bytes.len() > expected_total {
        return Err(ProtocolError::Invalid(format!(
            "帧尾存在 {} 字节多余数据",
            bytes.len() - expected_total
        )));
    }
    let limit = match kind {
        FrameKind::Handshake => MAX_HANDSHAKE_FRAME_BYTES,
        FrameKind::Encrypted => MAX_CONTROL_FRAME_BYTES,
    };
    if expected_total > limit {
        return Err(ProtocolError::FrameTooLarge {
            actual: expected_total,
            limit,
        });
    }
    Ok(ControlFrame {
        kind,
        seq,
        body: bytes[HEADER_LEN..].to_vec(),
    })
}

pub fn peek_frame_len(header: &[u8]) -> Option<usize> {
    if header.len() < 4 {
        return None;
    }
    Some(HEADER_LEN + u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize)
}
