use nc_core::{NexusError, Result};
use serde::{Deserialize, Serialize};

use crate::wire::{HEADER_LEN, MAX_DATAGRAM_BYTES, TAG_LEN};

pub const DEFAULT_MTU: usize = 1200;
pub const DEFAULT_FRAME_TIMEOUT_MS: u64 = 80;
pub const DEFAULT_NACK_DELAY_MS: u64 = 8;
pub const DEFAULT_REORDER_WINDOW_MS: u64 = 20;
pub const DEFAULT_MAX_PENDING_FRAMES: usize = 32;
pub(crate) const FEC_LENGTH_PREFIX_BYTES: usize = 4;
pub const MAX_FEC_PARITY: u8 = 1;
pub(crate) const MAX_FEC_TOTAL_SHARDS: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaConfig {
    pub mtu: usize,
    pub frame_timeout_ms: u64,
    pub nack_delay_ms: u64,
    pub reorder_window_ms: u64,
    pub fec_parity: u8,
    pub max_pending_frames: usize,
    #[serde(default)]
    pub session_id: u64,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            mtu: DEFAULT_MTU,
            frame_timeout_ms: DEFAULT_FRAME_TIMEOUT_MS,
            nack_delay_ms: DEFAULT_NACK_DELAY_MS,
            reorder_window_ms: DEFAULT_REORDER_WINDOW_MS,
            fec_parity: 0,
            max_pending_frames: DEFAULT_MAX_PENDING_FRAMES,
            session_id: 0,
        }
    }
}

impl MediaConfig {
    pub fn validate(&self) -> Result<()> {
        if self.mtu > MAX_DATAGRAM_BYTES {
            return Err(NexusError::InvalidInput(format!(
                "MTU {} 超过 UDP 单包上限 {MAX_DATAGRAM_BYTES}",
                self.mtu
            )));
        }
        if self.fec_parity > MAX_FEC_PARITY {
            return Err(NexusError::InvalidInput(format!(
                "FEC 校验分片数 {} 不受支持，当前仅支持 0 或 {MAX_FEC_PARITY}",
                self.fec_parity
            )));
        }
        if self.max_pending_frames == 0 {
            return Err(NexusError::InvalidInput(
                "max_pending_frames 必须大于 0".to_string(),
            ));
        }
        self.payload_capacity().map(|_| ())
    }

    pub const fn fec_enabled(&self) -> bool {
        self.fec_parity > 0
    }

    pub fn payload_capacity(&self) -> Result<usize> {
        let parity = if self.fec_enabled() {
            MAX_FEC_PARITY
        } else {
            0
        };
        self.payload_capacity_for_parity(parity)
    }

    pub(crate) fn payload_capacity_for_parity(&self, parity_count: u8) -> Result<usize> {
        let prefix = if parity_count > 0 {
            FEC_LENGTH_PREFIX_BYTES
        } else {
            0
        };
        let overhead = HEADER_LEN + TAG_LEN + prefix;
        self.mtu
            .checked_sub(overhead)
            .filter(|capacity| *capacity > 0)
            .ok_or_else(|| {
                NexusError::InvalidInput(format!(
                    "MTU {} 不足，至少需要 {} 字节",
                    self.mtu,
                    overhead + 1
                ))
            })
    }

    pub fn max_frame_bytes(&self) -> Result<usize> {
        self.validate()?;
        let capacity = self.payload_capacity()?;
        let max_fragments = if self.fec_enabled() {
            MAX_FEC_TOTAL_SHARDS - usize::from(self.fec_parity)
        } else {
            usize::from(u16::MAX)
        };
        Ok(capacity.saturating_mul(max_fragments))
    }
}

