use std::collections::HashMap;

use nc_core::{EncodedVideoFrame, NexusError, Result, StreamId};
use reed_solomon_erasure::galois_8::ReedSolomon;

use crate::config::{MediaConfig, FEC_LENGTH_PREFIX_BYTES, MAX_FEC_TOTAL_SHARDS};
use crate::wire::{
    random_nonce, MediaHeader, MediaPacket, PacketKind, FLAG_AUDIO, FLAG_FEC_PARITY, FLAG_KEYFRAME,
};

pub struct Fragmenter {
    config: MediaConfig,
    next_frame_id: HashMap<StreamId, u32>,
    next_packet_seq: HashMap<StreamId, u32>,
}

impl Fragmenter {
    pub fn new(config: MediaConfig) -> Self {
        Self {
            config,
            next_frame_id: HashMap::new(),
            next_packet_seq: HashMap::new(),
        }
    }

    pub fn config(&self) -> &MediaConfig {
        &self.config
    }

    pub fn set_fec_parity(&mut self, parity: u8) -> bool {
        if parity > crate::config::MAX_FEC_PARITY || parity == self.config.fec_parity {
            return false;
        }
        self.config.fec_parity = parity;
        true
    }

    pub fn fragment_video(
        &mut self,
        frame: &EncodedVideoFrame,
        stream_id: StreamId,
    ) -> Result<Vec<MediaPacket>> {
        self.fragment_frame(
            stream_id,
            frame.timestamp_us,
            &frame.data,
            frame.keyframe,
            false,
        )
    }

    pub fn fragment_audio(
        &mut self,
        timestamp_us: u64,
        data: impl AsRef<[u8]>,
        stream_id: StreamId,
    ) -> Result<Vec<MediaPacket>> {
        self.fragment_frame(stream_id, timestamp_us, data.as_ref(), false, true)
    }

    fn fragment_frame(
        &mut self,
        stream_id: StreamId,
        timestamp_us: u64,
        data: &[u8],
        keyframe: bool,
        is_audio: bool,
    ) -> Result<Vec<MediaPacket>> {
        self.config.validate()?;
        if data.len() > u32::MAX as usize {
            return Err(NexusError::InvalidInput(format!(
                "帧长度 {} 超过 4 GiB 上限",
                data.len()
            )));
        }
        let capacity = self.config.payload_capacity()?;
        let fragment_count = if data.is_empty() {
            1
        } else {
            data.len().div_ceil(capacity)
        };
        if fragment_count > usize::from(u16::MAX) {
            return Err(NexusError::InvalidInput(format!(
                "帧需要 {fragment_count} 个分片，超过 u16 上限"
            )));
        }
        if data.len() > self.config.max_frame_bytes()? {
            return Err(NexusError::InvalidInput(format!(
                "帧长度 {} 超过当前配置上限 {}",
                data.len(),
                self.config.max_frame_bytes()?
            )));
        }
        let parity_count = usize::from(self.config.fec_parity);
        if parity_count > 0 && fragment_count + parity_count > MAX_FEC_TOTAL_SHARDS {
            return Err(NexusError::InvalidInput(format!(
                "FEC 分片总数 {} 超过上限 {MAX_FEC_TOTAL_SHARDS}",
                fragment_count + parity_count
            )));
        }

        let frame_id = *self.next_frame_id.entry(stream_id).or_insert(1);
        let start_seq = *self.next_packet_seq.entry(stream_id).or_insert(0);
        let packet_count = fragment_count + parity_count;

        let mut base_flags = 0u8;
        if keyframe {
            base_flags |= FLAG_KEYFRAME;
        }
        if is_audio {
            base_flags |= FLAG_AUDIO;
        }

        let mut packets = Vec::with_capacity(packet_count);
        for index in 0..fragment_count {
            let start = index * capacity;
            let end = (start + capacity).min(data.len());
            let payload = data[start..end].to_vec();
            packets.push(MediaPacket::new(
                MediaHeader {
                    kind: PacketKind::Data,
                    nonce: random_nonce(),
                    flags: base_flags,
                    parity_count: parity_count as u8,
                    session_id: self.config.session_id,
                    stream_id,
                    frame_id,
                    fragment_index: index as u16,
                    fragment_count: fragment_count as u16,
                    packet_seq: start_seq.wrapping_add(index as u32),
                    timestamp_us,
                },
                payload,
            ));
        }

        if parity_count > 0 {
            let parity_shards = encode_parity_shards(data, fragment_count, capacity, parity_count)?;
            let length_prefix = (data.len() as u32).to_le_bytes();
            for (index, shard) in parity_shards.into_iter().enumerate() {
                let mut payload = Vec::with_capacity(FEC_LENGTH_PREFIX_BYTES + shard.len());
                payload.extend_from_slice(&length_prefix);
                payload.extend_from_slice(&shard);
                packets.push(MediaPacket::new(
                    MediaHeader {
                        kind: PacketKind::Data,
                        nonce: random_nonce(),
                        flags: base_flags | FLAG_FEC_PARITY,
                        parity_count: parity_count as u8,
                        session_id: self.config.session_id,
                        stream_id,
                        frame_id,
                        fragment_index: index as u16,
                        fragment_count: fragment_count as u16,
                        packet_seq: start_seq.wrapping_add((fragment_count + index) as u32),
                        timestamp_us,
                    },
                    payload,
                ));
            }
        }

        self.next_frame_id
            .insert(stream_id, frame_id.wrapping_add(1));
        self.next_packet_seq
            .insert(stream_id, start_seq.wrapping_add(packet_count as u32));
        Ok(packets)
    }
}

fn encode_parity_shards(
    data: &[u8],
    data_shards: usize,
    shard_len: usize,
    parity_shards: usize,
) -> Result<Vec<Vec<u8>>> {
    let codec = ReedSolomon::new(data_shards, parity_shards)
        .map_err(|error| NexusError::Media(format!("FEC 编码器创建失败：{error}")))?;
    let mut shards = Vec::with_capacity(data_shards + parity_shards);
    for index in 0..data_shards {
        let start = index * shard_len;
        let end = (start + shard_len).min(data.len());
        let mut shard = vec![0u8; shard_len];
        let copy_len = end.saturating_sub(start);
        if copy_len > 0 {
            shard[..copy_len].copy_from_slice(&data[start..end]);
        }
        shards.push(shard);
    }
    for _ in 0..parity_shards {
        shards.push(vec![0u8; shard_len]);
    }
    let mut shard_refs: Vec<&mut [u8]> = shards.iter_mut().map(Vec::as_mut_slice).collect();
    codec
        .encode(&mut shard_refs)
        .map_err(|error| NexusError::Media(format!("FEC 编码失败：{error}")))?;
    Ok(shards.split_off(data_shards))
}
