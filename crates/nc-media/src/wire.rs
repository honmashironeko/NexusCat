use bincode::Options;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use nc_core::{NexusError, Result, StreamId};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::key::MediaKey;

pub const MAGIC: u32 = 0x314D_434E;
pub const WIRE_VERSION: u8 = 1;
pub const HEADER_LEN: usize = 50;
pub const AAD_LEN: usize = 38;
pub const TAG_LEN: usize = 16;
pub const MAX_DATAGRAM_BYTES: usize = 65_507;
pub const FLAG_KEYFRAME: u8 = 1 << 0;
pub const FLAG_AUDIO: u8 = 1 << 1;
pub const FLAG_FEC_PARITY: u8 = 1 << 2;
pub const MAX_NACK_MISSING: usize = 4096;
pub const MAX_NACK_BODY_BYTES: usize = 16 * 1024;

const KNOWN_FLAGS: u8 = FLAG_KEYFRAME | FLAG_AUDIO | FLAG_FEC_PARITY;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PacketKind {
    #[default]
    Data,
    Nack,
}

impl PacketKind {
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::Data => 0,
            Self::Nack => 1,
        }
    }

    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Data),
            1 => Some(Self::Nack),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaHeader {
    pub kind: PacketKind,
    pub flags: u8,
    pub parity_count: u8,
    pub session_id: u64,
    pub stream_id: StreamId,
    pub frame_id: u32,
    pub fragment_index: u16,
    pub fragment_count: u16,
    pub packet_seq: u32,
    pub timestamp_us: u64,
    pub nonce: [u8; 12],
}

impl Default for MediaHeader {
    fn default() -> Self {
        Self {
            kind: PacketKind::Data,
            flags: 0,
            parity_count: 0,
            session_id: 0,
            stream_id: StreamId(0),
            frame_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            packet_seq: 0,
            timestamp_us: 0,
            nonce: [0u8; 12],
        }
    }
}

impl MediaHeader {
    pub fn data(stream_id: StreamId, frame_id: u32) -> Self {
        Self {
            kind: PacketKind::Data,
            stream_id,
            frame_id,
            ..Self::default()
        }
    }

    pub fn nack(stream_id: StreamId, frame_id: u32) -> Self {
        Self {
            kind: PacketKind::Nack,
            stream_id,
            frame_id,
            ..Self::default()
        }
    }

    pub const fn is_keyframe(&self) -> bool {
        self.flags & FLAG_KEYFRAME != 0
    }

    pub const fn is_audio(&self) -> bool {
        self.flags & FLAG_AUDIO != 0
    }

    pub const fn is_parity(&self) -> bool {
        self.flags & FLAG_FEC_PARITY != 0
    }

    pub(crate) fn validate(&self) -> std::result::Result<(), String> {
        if self.flags & !KNOWN_FLAGS != 0 {
            return Err(format!("未知标志位 0x{:02x}", self.flags & !KNOWN_FLAGS));
        }
        match self.kind {
            PacketKind::Data => {
                if self.fragment_count == 0 {
                    return Err("数据包 fragment_count 不能为 0".to_string());
                }
                if self.is_parity() {
                    if self.parity_count == 0 {
                        return Err("校验分片必须声明 parity_count".to_string());
                    }
                    if self.fragment_index >= u16::from(self.parity_count) {
                        return Err(format!(
                            "校验分片索引 {} 超出 parity_count {}",
                            self.fragment_index, self.parity_count
                        ));
                    }
                } else if self.fragment_index >= self.fragment_count {
                    return Err(format!(
                        "数据分片索引 {} 超出 fragment_count {}",
                        self.fragment_index, self.fragment_count
                    ));
                }
                if self.parity_count > 0
                    && usize::from(self.fragment_count) + usize::from(self.parity_count)
                        > crate::config::MAX_FEC_TOTAL_SHARDS
                {
                    return Err("FEC 分片总数超过 255".to_string());
                }
            }
            PacketKind::Nack => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NackBody {
    pub frame_id: u32,
    pub missing: Vec<u16>,
}

impl NackBody {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.missing.len() > MAX_NACK_MISSING {
            return Err(NexusError::InvalidInput(format!(
                "NACK 缺片数量 {} 超过上限 {MAX_NACK_MISSING}",
                self.missing.len()
            )));
        }
        let bytes = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .serialize(self)
            .map_err(|error| NexusError::Media(format!("NACK 序列化失败：{error}")))?;
        if bytes.len() > MAX_NACK_BODY_BYTES {
            return Err(NexusError::InvalidInput(format!(
                "NACK 载荷 {} 字节超过上限 {MAX_NACK_BODY_BYTES}",
                bytes.len()
            )));
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let body: Self = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(MAX_NACK_BODY_BYTES as u64)
            .reject_trailing_bytes()
            .deserialize(bytes)
            .map_err(|error| NexusError::Media(format!("NACK 载荷解码失败：{error}")))?;
        if body.missing.len() > MAX_NACK_MISSING {
            return Err(NexusError::Media(format!(
                "NACK 缺片数量 {} 超过上限 {MAX_NACK_MISSING}",
                body.missing.len()
            )));
        }
        Ok(body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPacket {
    pub header: MediaHeader,
    pub payload: Vec<u8>,
}

impl MediaPacket {
    pub fn new(header: MediaHeader, payload: Vec<u8>) -> Self {
        Self { header, payload }
    }

    pub fn encode(&self, key: &MediaKey) -> Result<Vec<u8>> {
        self.header.validate().map_err(NexusError::InvalidInput)?;
        if self.header.kind == PacketKind::Data
            && self.header.is_parity()
            && self.payload.len() < crate::config::FEC_LENGTH_PREFIX_BYTES
        {
            return Err(NexusError::InvalidInput(
                "FEC 校验分片缺少原始长度前缀".to_string(),
            ));
        }
        if self.header.kind == PacketKind::Nack {
            NackBody::decode(&self.payload)
                .map_err(|error| NexusError::InvalidInput(format!("NACK 载荷非法：{error}")))?;
        }

        let mut bytes = [0u8; HEADER_LEN];
        bytes[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4] = WIRE_VERSION;
        bytes[5] = self.header.kind.to_u8();
        bytes[6] = self.header.flags;
        bytes[7] = self.header.parity_count;
        bytes[8..16].copy_from_slice(&self.header.session_id.to_le_bytes());
        bytes[16..18].copy_from_slice(&self.header.stream_id.0.to_le_bytes());
        bytes[18..22].copy_from_slice(&self.header.frame_id.to_le_bytes());
        bytes[22..24].copy_from_slice(&self.header.fragment_index.to_le_bytes());
        bytes[24..26].copy_from_slice(&self.header.fragment_count.to_le_bytes());
        bytes[26..30].copy_from_slice(&self.header.packet_seq.to_le_bytes());
        bytes[30..38].copy_from_slice(&self.header.timestamp_us.to_le_bytes());
        bytes[38..50].copy_from_slice(&self.header.nonce);

        let cipher = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(key.as_bytes()));
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&self.header.nonce),
                Payload {
                    msg: &self.payload,
                    aad: &bytes[..AAD_LEN],
                },
            )
            .map_err(|_| NexusError::Media("媒体包加密失败".to_string()))?;

        let total = HEADER_LEN + ciphertext.len();
        if total > MAX_DATAGRAM_BYTES {
            return Err(NexusError::InvalidInput(format!(
                "媒体包 {total} 字节超过 UDP 上限 {MAX_DATAGRAM_BYTES}"
            )));
        }
        let mut output = Vec::with_capacity(total);
        output.extend_from_slice(&bytes);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    pub fn decode(bytes: &[u8], key: &MediaKey) -> Result<Self> {
        if bytes.len() < HEADER_LEN + TAG_LEN {
            return Err(NexusError::Media(format!(
                "媒体包长度 {} 小于最小长度 {}",
                bytes.len(),
                HEADER_LEN + TAG_LEN
            )));
        }
        if bytes.len() > MAX_DATAGRAM_BYTES {
            return Err(NexusError::Media(format!(
                "媒体包长度 {} 超过 UDP 上限",
                bytes.len()
            )));
        }

        let magic = u32::from_le_bytes(bytes[0..4].try_into().expect("固定切片长度"));
        if magic != MAGIC {
            return Err(NexusError::Media(format!("媒体包魔数错误：0x{magic:08x}")));
        }
        if bytes[4] != WIRE_VERSION {
            return Err(NexusError::Media(format!(
                "媒体包版本 {} 不受支持",
                bytes[4]
            )));
        }
        let kind = PacketKind::from_u8(bytes[5])
            .ok_or_else(|| NexusError::Media(format!("未知媒体包类型 {}", bytes[5])))?;

        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&bytes[38..50]);
        let header = MediaHeader {
            kind,
            flags: bytes[6],
            parity_count: bytes[7],
            session_id: u64::from_le_bytes(bytes[8..16].try_into().expect("固定切片长度")),
            stream_id: StreamId(u16::from_le_bytes(
                bytes[16..18].try_into().expect("固定切片长度"),
            )),
            frame_id: u32::from_le_bytes(bytes[18..22].try_into().expect("固定切片长度")),
            fragment_index: u16::from_le_bytes(bytes[22..24].try_into().expect("固定切片长度")),
            fragment_count: u16::from_le_bytes(bytes[24..26].try_into().expect("固定切片长度")),
            packet_seq: u32::from_le_bytes(bytes[26..30].try_into().expect("固定切片长度")),
            timestamp_us: u64::from_le_bytes(bytes[30..38].try_into().expect("固定切片长度")),
            nonce,
        };
        header
            .validate()
            .map_err(|message| NexusError::Media(format!("媒体包头非法：{message}")))?;

        let cipher = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(key.as_bytes()));
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &bytes[HEADER_LEN..],
                    aad: &bytes[..AAD_LEN],
                },
            )
            .map_err(|_| NexusError::Media("媒体包认证或解密失败".to_string()))?;

        if kind == PacketKind::Nack {
            NackBody::decode(&plaintext)?;
        }
        Ok(Self {
            header,
            payload: plaintext,
        })
    }

    pub fn from_nack(session_id: u64, stream_id: StreamId, body: &NackBody) -> Result<Self> {
        let header = MediaHeader {
            session_id,
            nonce: random_nonce(),
            ..MediaHeader::nack(stream_id, body.frame_id)
        };
        Ok(Self {
            header,
            payload: body.encode()?,
        })
    }

    pub fn nack_body(&self) -> Result<NackBody> {
        if self.header.kind != PacketKind::Nack {
            return Err(NexusError::InvalidInput(
                "当前媒体包不是 NACK 包".to_string(),
            ));
        }
        NackBody::decode(&self.payload)
    }
}

pub(crate) fn random_nonce() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    nonce
}
