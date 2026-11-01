use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use nc_core::StreamId;
use reed_solomon_erasure::galois_8::ReedSolomon;

use crate::config::{MediaConfig, FEC_LENGTH_PREFIX_BYTES};
use crate::stats::MediaStats;
use crate::wire::{MediaHeader, MediaPacket, PacketKind};

const SEQ_WINDOW_BITS: usize = 4096;
const SEQ_WINDOW_WORDS: usize = SEQ_WINDOW_BITS / 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledFrame {
    pub stream_id: StreamId,
    pub frame_id: u32,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
    pub is_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NackRequest {
    pub stream_id: StreamId,
    pub frame_id: u32,
    pub missing: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassemblyEvent {
    Pending,
    Complete(AssembledFrame),
    Duplicate,
    Rejected(String),
}

pub struct Reassembler {
    config: MediaConfig,
    streams: BTreeMap<StreamId, StreamState>,
    nacks: Vec<NackRequest>,
    stats: StatsCounters,
    stale_frames: BTreeSet<(StreamId, u32)>,
}

impl Reassembler {
    pub fn new(config: MediaConfig) -> Self {
        Self {
            config,
            streams: BTreeMap::new(),
            nacks: Vec::new(),
            stats: StatsCounters::default(),
            stale_frames: BTreeSet::new(),
        }
    }

    pub fn config(&self) -> &MediaConfig {
        &self.config
    }

    pub fn push(&mut self, packet: MediaPacket, now: Instant) -> ReassemblyEvent {
        if let Err(error) = self.config.validate() {
            return ReassemblyEvent::Rejected(error.to_string());
        }
        self.drop_expired_frames(now);
        let MediaPacket { header, payload } = packet;
        if header.kind != PacketKind::Data {
            return ReassemblyEvent::Rejected("重组器不接受 NACK 包".to_string());
        }
        if let Err(message) = header.validate() {
            return ReassemblyEvent::Rejected(format!("媒体包头非法：{message}"));
        }
        let capacity = match self.config.payload_capacity_for_parity(header.parity_count) {
            Ok(capacity) => capacity,
            Err(error) => return ReassemblyEvent::Rejected(error.to_string()),
        };
        let parsed = match parse_fragment_payload(&header, payload, capacity) {
            Ok(parsed) => parsed,
            Err(message) => return ReassemblyEvent::Rejected(message),
        };

        let stream_id = header.stream_id;
        let fresh_sequence = {
            let state = self.streams.entry(stream_id).or_default();
            state.sequence.observe(header.packet_seq)
        };
        if !fresh_sequence {
            self.stats.duplicate_packets += 1;
            return ReassemblyEvent::Duplicate;
        }
        if let Some(last) = self
            .streams
            .get(&stream_id)
            .and_then(|state| state.last_emitted)
        {
            if header.frame_id <= last {
                self.note_stale_frame(stream_id, header.frame_id);
                return ReassemblyEvent::Rejected(format!(
                    "帧 {} 早于已输出帧 {last}",
                    header.frame_id
                ));
            }
        }

        let frame_id = header.frame_id;
        let exists = self
            .streams
            .get(&stream_id)
            .is_some_and(|state| state.pending.contains_key(&frame_id));
        if !exists {
            self.evict_oldest_if_full();
            let pending = PendingFrame::new(&header, now);
            let state = self.streams.get_mut(&stream_id).expect("流状态已创建");
            state.pending.insert(frame_id, pending);
        }

        let store_result = {
            let state = self.streams.get_mut(&stream_id).expect("流状态已创建");
            let pending = state.pending.get_mut(&frame_id).expect("待重组帧已创建");
            pending.store(&header, parsed, capacity)
        };

        match store_result {
            StoreResult::Duplicate => {
                self.stats.duplicate_packets += 1;
                return ReassemblyEvent::Duplicate;
            }
            StoreResult::Inconsistent(message) => {
                if let Some(pending) = self.remove_pending(stream_id, frame_id) {
                    self.record_drop(&pending);
                }
                return ReassemblyEvent::Rejected(message);
            }
            StoreResult::Stored => {}
        }

        let should_attempt = {
            let state = self.streams.get(&stream_id).expect("流状态已创建");
            let pending = state.pending.get(&frame_id).expect("待重组帧已创建");
            let missing = pending.missing_count();
            missing == 0 || (pending.parity_count > 0 && pending.present_parity() >= missing)
        };

        if !should_attempt {
            self.generate_nacks_for_stream(stream_id, now);
            if let Some(frame) = self.drain_stream(stream_id, now, 1).into_iter().next() {
                self.drop_stale_pending(stream_id);
                return ReassemblyEvent::Complete(frame);
            }
            return ReassemblyEvent::Pending;
        }

        let Some(pending) = self.remove_pending(stream_id, frame_id) else {
            return ReassemblyEvent::Pending;
        };
        match attempt_complete(&self.config, &pending) {
            CompleteAttempt::Complete { data, missing } => {
                self.stats.add_frame(
                    u64::from(pending.fragment_count),
                    u64::try_from(missing).unwrap_or(u64::MAX),
                );
                self.queue_completed(stream_id, frame_id, pending, data, now)
            }
            CompleteAttempt::Corrupt(message) => {
                self.record_drop(&pending);
                ReassemblyEvent::Rejected(message)
            }
            CompleteAttempt::Incomplete => {
                let state = self.streams.get_mut(&stream_id).expect("流状态已创建");
                state.pending.insert(frame_id, pending);
                self.generate_nacks_for_stream(stream_id, now);
                ReassemblyEvent::Pending
            }
        }
    }

    pub fn take_nacks(&mut self) -> Vec<NackRequest> {
        std::mem::take(&mut self.nacks)
    }

    pub fn poll(&mut self, now: Instant) -> Vec<ReassemblyEvent> {
        let mut events = Vec::new();
        if self.config.validate().is_err() {
            return events;
        }

        let timeout = Duration::from_millis(self.config.frame_timeout_ms);
        let mut timed_out = Vec::new();
        for (&stream_id, state) in self.streams.iter_mut() {
            let expired: Vec<u32> = state
                .pending
                .iter()
                .filter(|(_, pending)| now.saturating_duration_since(pending.first_seen) >= timeout)
                .map(|(frame_id, _)| *frame_id)
                .collect();
            for frame_id in expired {
                if let Some(pending) = state.pending.remove(&frame_id) {
                    timed_out.push((stream_id, pending));
                }
            }
        }

        for (stream_id, pending) in timed_out {
            let frame_id = pending.frame_id;
            match attempt_complete(&self.config, &pending) {
                CompleteAttempt::Complete { data, missing } => {
                    self.stats.add_frame(
                        u64::from(pending.fragment_count),
                        u64::try_from(missing).unwrap_or(u64::MAX),
                    );
                    match self.queue_completed(stream_id, frame_id, pending, data, now) {
                        ReassemblyEvent::Complete(frame) => {
                            events.push(ReassemblyEvent::Complete(frame));
                        }
                        ReassemblyEvent::Rejected(message) => {
                            events.push(ReassemblyEvent::Rejected(message));
                        }
                        ReassemblyEvent::Pending | ReassemblyEvent::Duplicate => {}
                    }
                }
                CompleteAttempt::Incomplete | CompleteAttempt::Corrupt(_) => {
                    self.record_drop(&pending);
                    events.push(ReassemblyEvent::Rejected(format!("帧 {frame_id} 重组超时")));
                }
            }
        }

        let stream_ids: Vec<StreamId> = self.streams.keys().copied().collect();
        for stream_id in stream_ids {
            for frame in self.drain_stream(stream_id, now, usize::MAX) {
                events.push(ReassemblyEvent::Complete(frame));
            }
            self.drop_stale_pending(stream_id);
        }
        for stream_id in self.streams.keys().copied().collect::<Vec<_>>() {
            self.generate_nacks_for_stream(stream_id, now);
        }
        events
    }

    pub fn stats(&self) -> MediaStats {
        self.stats.snapshot()
    }

    pub fn pending_frames(&self) -> usize {
        self.streams.values().map(|state| state.pending.len()).sum()
    }

    fn drop_expired_frames(&mut self, now: Instant) {
        let timeout = Duration::from_millis(self.config.frame_timeout_ms);
        let mut expired = Vec::new();
        for (&stream_id, state) in self.streams.iter_mut() {
            let ids: Vec<u32> = state
                .pending
                .iter()
                .filter(|(_, pending)| now.saturating_duration_since(pending.first_seen) >= timeout)
                .map(|(frame_id, _)| *frame_id)
                .collect();
            for frame_id in ids {
                if let Some(pending) = state.pending.remove(&frame_id) {
                    expired.push((stream_id, pending));
                }
            }
        }
        for (_, pending) in expired {
            self.record_drop(&pending);
        }
    }

    fn note_stale_frame(&mut self, stream_id: StreamId, frame_id: u32) {
        if self.stale_frames.insert((stream_id, frame_id)) {
            self.stats.dropped_frames += 1;
            if self.stale_frames.len() > 256 {
                self.stale_frames.clear();
            }
        }
    }

    fn remove_pending(&mut self, stream_id: StreamId, frame_id: u32) -> Option<PendingFrame> {
        self.streams
            .get_mut(&stream_id)
            .and_then(|state| state.pending.remove(&frame_id))
    }

    fn record_drop(&mut self, pending: &PendingFrame) {
        self.stats.add_frame(
            u64::from(pending.fragment_count),
            u64::try_from(pending.missing_count()).unwrap_or(u64::MAX),
        );
        self.stats.dropped_frames += 1;
    }

    fn queue_completed(
        &mut self,
        stream_id: StreamId,
        frame_id: u32,
        pending: PendingFrame,
        data: Vec<u8>,
        now: Instant,
    ) -> ReassemblyEvent {
        let outdated = self
            .streams
            .get(&stream_id)
            .and_then(|state| state.last_emitted)
            .is_some_and(|last| frame_id <= last);
        if outdated {
            self.stats.dropped_frames += 1;
            return ReassemblyEvent::Rejected(format!("帧 {frame_id} 已过期"));
        }
        let state = self.streams.get_mut(&stream_id).expect("流状态已创建");
        state.ready.insert(
            frame_id,
            ReadyFrame {
                frame: AssembledFrame {
                    stream_id,
                    frame_id,
                    timestamp_us: pending.timestamp_us,
                    keyframe: pending.keyframe,
                    data,
                    is_audio: pending.is_audio,
                },
                completed_at: now,
                min_packet_seq: pending.min_packet_seq,
            },
        );
        match self.drain_stream(stream_id, now, 1).into_iter().next() {
            Some(frame) => {
                self.drop_stale_pending(stream_id);
                ReassemblyEvent::Complete(frame)
            }
            None => ReassemblyEvent::Pending,
        }
    }

    fn drain_stream(
        &mut self,
        stream_id: StreamId,
        now: Instant,
        limit: usize,
    ) -> Vec<AssembledFrame> {
        let window = Duration::from_millis(self.config.reorder_window_ms);
        let stats = &mut self.stats;
        let mut emitted = Vec::new();
        let Some(state) = self.streams.get_mut(&stream_id) else {
            return emitted;
        };
        while emitted.len() < limit {
            let Some(frame_id) = state.ready.keys().next().copied() else {
                break;
            };
            let Some(ready) = state.ready.get(&frame_id) else {
                break;
            };
            let elapsed = now.saturating_duration_since(ready.completed_at);
            let can_emit = match state.last_emitted {
                None => frame_id <= 1 || ready.min_packet_seq == 0 || elapsed >= window,
                Some(last) if frame_id <= last => {
                    state.ready.remove(&frame_id);
                    stats.dropped_frames += 1;
                    continue;
                }
                Some(last) if frame_id == last.wrapping_add(1) => true,
                Some(_) => elapsed >= window,
            };
            if !can_emit {
                break;
            }
            let Some(ready) = state.ready.remove(&frame_id) else {
                break;
            };
            state.last_emitted = Some(frame_id);
            stats.frames_received += 1;
            emitted.push(ready.frame);
        }
        emitted
    }

    fn drop_stale_pending(&mut self, stream_id: StreamId) {
        let stats = &mut self.stats;
        let Some(state) = self.streams.get_mut(&stream_id) else {
            return;
        };
        let Some(last) = state.last_emitted else {
            return;
        };
        let stale: Vec<u32> = state
            .pending
            .range(..=last)
            .map(|(frame_id, _)| *frame_id)
            .collect();
        for frame_id in stale {
            if let Some(pending) = state.pending.remove(&frame_id) {
                stats.add_frame(
                    u64::from(pending.fragment_count),
                    u64::try_from(pending.missing_count()).unwrap_or(u64::MAX),
                );
                stats.dropped_frames += 1;
            }
        }
    }

    fn evict_oldest_if_full(&mut self) {
        let limit = self.config.max_pending_frames.max(1);
        if self.pending_frames() < limit {
            return;
        }
        let oldest = self
            .streams
            .iter()
            .flat_map(|(&stream_id, state)| {
                state
                    .pending
                    .iter()
                    .map(move |(&frame_id, pending)| (pending.first_seen, stream_id, frame_id))
            })
            .min_by_key(|(first_seen, _, _)| *first_seen);
        if let Some((_, stream_id, frame_id)) = oldest {
            if let Some(pending) = self.remove_pending(stream_id, frame_id) {
                self.record_drop(&pending);
            }
        }
    }

    fn generate_nacks_for_stream(&mut self, stream_id: StreamId, now: Instant) {
        let delay = Duration::from_millis(self.config.nack_delay_ms);
        let mut requests = Vec::new();
        if let Some(state) = self.streams.get_mut(&stream_id) {
            for (&frame_id, pending) in state.pending.iter_mut() {
                let missing = pending.missing_indices();
                if missing.is_empty() {
                    continue;
                }
                if now.saturating_duration_since(pending.first_seen) < delay {
                    continue;
                }
                if let Some(last) = pending.last_nack_at {
                    if now.saturating_duration_since(last) < delay {
                        continue;
                    }
                }
                pending.last_nack_at = Some(now);
                requests.push(NackRequest {
                    stream_id,
                    frame_id,
                    missing,
                });
            }
        }
        self.nacks.extend(requests);
    }
}

#[derive(Default)]
struct StreamState {
    pending: BTreeMap<u32, PendingFrame>,
    ready: BTreeMap<u32, ReadyFrame>,
    last_emitted: Option<u32>,
    sequence: SequenceWindow,
}

struct ReadyFrame {
    frame: AssembledFrame,
    completed_at: Instant,
    min_packet_seq: u32,
}

struct PendingFrame {
    frame_id: u32,
    timestamp_us: u64,
    keyframe: bool,
    is_audio: bool,
    fragment_count: u16,
    parity_count: u8,
    data: Vec<Option<Vec<u8>>>,
    parity: Vec<Option<Vec<u8>>>,
    parity_frame_len: Option<u32>,
    first_seen: Instant,
    last_nack_at: Option<Instant>,
    min_packet_seq: u32,
}

impl PendingFrame {
    fn new(header: &MediaHeader, now: Instant) -> Self {
        Self {
            frame_id: header.frame_id,
            timestamp_us: header.timestamp_us,
            keyframe: header.is_keyframe(),
            is_audio: header.is_audio(),
            fragment_count: header.fragment_count,
            parity_count: header.parity_count,
            data: vec![None; usize::from(header.fragment_count)],
            parity: vec![None; usize::from(header.parity_count)],
            parity_frame_len: None,
            first_seen: now,
            last_nack_at: None,
            min_packet_seq: header.packet_seq,
        }
    }

    fn store(
        &mut self,
        header: &MediaHeader,
        payload: FragmentPayload,
        capacity: usize,
    ) -> StoreResult {
        if self.fragment_count != header.fragment_count
            || self.parity_count != header.parity_count
            || self.timestamp_us != header.timestamp_us
            || self.keyframe != header.is_keyframe()
            || self.is_audio != header.is_audio()
        {
            return StoreResult::Inconsistent(format!("帧 {} 的分片元数据不一致", self.frame_id));
        }
        self.min_packet_seq = self.min_packet_seq.min(header.packet_seq);
        let index = usize::from(header.fragment_index);
        match payload {
            FragmentPayload::Data(bytes) => {
                let Some(slot) = self.data.get_mut(index) else {
                    return StoreResult::Inconsistent("数据分片索引越界".to_string());
                };
                if slot.is_some() {
                    return StoreResult::Duplicate;
                }
                if bytes.len() > capacity {
                    return StoreResult::Inconsistent("数据分片超过 MTU 容量".to_string());
                }
                *slot = Some(bytes);
            }
            FragmentPayload::Parity { shard, frame_len } => {
                let Some(slot) = self.parity.get_mut(index) else {
                    return StoreResult::Inconsistent("校验分片索引越界".to_string());
                };
                if slot.is_some() {
                    return StoreResult::Duplicate;
                }
                if shard.len() != capacity {
                    return StoreResult::Inconsistent("校验分片长度与 MTU 不符".to_string());
                }
                let max_len = usize::from(self.fragment_count).saturating_mul(capacity);
                let frame_len_usize = frame_len as usize;
                if frame_len_usize > max_len
                    || (self.fragment_count > 1
                        && frame_len_usize
                            <= usize::from(self.fragment_count - 1).saturating_mul(capacity))
                {
                    return StoreResult::Inconsistent("FEC 原始长度与分片数不符".to_string());
                }
                if let Some(existing) = self.parity_frame_len {
                    if existing != frame_len {
                        return StoreResult::Inconsistent(
                            "同一帧的校验分片长度前缀不一致".to_string(),
                        );
                    }
                }
                self.parity_frame_len = Some(frame_len);
                *slot = Some(shard);
            }
        }
        StoreResult::Stored
    }

    fn missing_count(&self) -> usize {
        self.data.iter().filter(|slot| slot.is_none()).count()
    }

    fn missing_indices(&self) -> Vec<u16> {
        self.data
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.is_none())
            .map(|(index, _)| index as u16)
            .collect()
    }

    fn present_parity(&self) -> usize {
        self.parity.iter().filter(|slot| slot.is_some()).count()
    }
}

enum FragmentPayload {
    Data(Vec<u8>),
    Parity { shard: Vec<u8>, frame_len: u32 },
}

fn parse_fragment_payload(
    header: &MediaHeader,
    payload: Vec<u8>,
    capacity: usize,
) -> std::result::Result<FragmentPayload, String> {
    if header.is_parity() {
        if payload.len() != FEC_LENGTH_PREFIX_BYTES + capacity {
            return Err(format!(
                "校验分片载荷 {} 字节，期望 {} 字节",
                payload.len(),
                FEC_LENGTH_PREFIX_BYTES + capacity
            ));
        }
        let frame_len = u32::from_le_bytes(
            payload[..FEC_LENGTH_PREFIX_BYTES]
                .try_into()
                .expect("固定长度前缀"),
        );
        Ok(FragmentPayload::Parity {
            shard: payload[FEC_LENGTH_PREFIX_BYTES..].to_vec(),
            frame_len,
        })
    } else {
        if payload.len() > capacity {
            return Err(format!(
                "数据分片载荷 {} 字节超过容量 {capacity}",
                payload.len()
            ));
        }
        Ok(FragmentPayload::Data(payload))
    }
}

enum StoreResult {
    Stored,
    Duplicate,
    Inconsistent(String),
}

enum CompleteAttempt {
    Complete { data: Vec<u8>, missing: usize },
    Incomplete,
    Corrupt(String),
}

fn attempt_complete(config: &MediaConfig, pending: &PendingFrame) -> CompleteAttempt {
    let fragment_count = usize::from(pending.fragment_count);
    let missing = pending.missing_count();
    if missing == 0 {
        let mut data = Vec::new();
        for slot in &pending.data {
            let Some(bytes) = slot else {
                return CompleteAttempt::Incomplete;
            };
            data.extend_from_slice(bytes);
        }
        if pending.parity_count > 0 {
            if let Some(expected_len) = pending.parity_frame_len {
                if usize::from(pending.fragment_count) > 0 && expected_len as usize != data.len() {
                    return CompleteAttempt::Corrupt(
                        "FEC 长度前缀与数据分片总长不一致".to_string(),
                    );
                }
            }
        }
        return CompleteAttempt::Complete { data, missing };
    }

    if pending.parity_count == 0 || pending.present_parity() < missing {
        return CompleteAttempt::Incomplete;
    }
    let capacity = match config.payload_capacity_for_parity(pending.parity_count) {
        Ok(capacity) => capacity,
        Err(error) => return CompleteAttempt::Corrupt(error.to_string()),
    };
    let Some(frame_len) = pending.parity_frame_len else {
        return CompleteAttempt::Incomplete;
    };
    let frame_len = frame_len as usize;
    let max_len = fragment_count.saturating_mul(capacity);
    if frame_len > max_len {
        return CompleteAttempt::Corrupt("FEC 原始长度超过分片容量".to_string());
    }
    if fragment_count > 1
        && frame_len <= usize::from(pending.fragment_count - 1).saturating_mul(capacity)
    {
        return CompleteAttempt::Corrupt("FEC 原始长度小于前置分片总长".to_string());
    }
    let last_index = fragment_count - 1;
    if let Some(last) = &pending.data[last_index] {
        let expected_last = frame_len - (fragment_count - 1).saturating_mul(capacity);
        if last.len() != expected_last {
            return CompleteAttempt::Corrupt("末片长度与 FEC 长度前缀不符".to_string());
        }
    }

    let mut shards: Vec<Option<Vec<u8>>> =
        Vec::with_capacity(fragment_count + usize::from(pending.parity_count));
    for slot in &pending.data {
        shards.push(match slot {
            Some(bytes) => {
                if bytes.len() > capacity {
                    return CompleteAttempt::Corrupt("数据分片超过 FEC 分片长度".to_string());
                }
                let mut padded = bytes.clone();
                padded.resize(capacity, 0);
                Some(padded)
            }
            None => None,
        });
    }
    for slot in &pending.parity {
        shards.push(match slot {
            Some(shard) => {
                if shard.len() != capacity {
                    return CompleteAttempt::Corrupt("校验分片长度不一致".to_string());
                }
                Some(shard.clone())
            }
            None => None,
        });
    }

    let codec = match ReedSolomon::new(fragment_count, usize::from(pending.parity_count)) {
        Ok(codec) => codec,
        Err(error) => return CompleteAttempt::Corrupt(format!("FEC 解码器创建失败：{error}")),
    };
    if let Err(error) = codec.reconstruct(&mut shards) {
        return CompleteAttempt::Corrupt(format!("FEC 重建失败：{error}"));
    }
    let mut data = Vec::with_capacity(fragment_count.saturating_mul(capacity));
    for shard in shards.iter().take(fragment_count) {
        let Some(shard) = shard else {
            return CompleteAttempt::Corrupt("FEC 重建后仍有缺失分片".to_string());
        };
        data.extend_from_slice(shard);
    }
    data.truncate(frame_len);
    CompleteAttempt::Complete { data, missing }
}

#[derive(Default)]
struct StatsCounters {
    frames_received: u64,
    dropped_frames: u64,
    duplicate_packets: u64,
    expected_fragments: u64,
    lost_fragments: u64,
}

impl StatsCounters {
    fn add_frame(&mut self, expected: u64, lost: u64) {
        self.expected_fragments = self.expected_fragments.saturating_add(expected);
        self.lost_fragments = self.lost_fragments.saturating_add(lost);
    }

    fn snapshot(&self) -> MediaStats {
        let mut stats = MediaStats {
            frames_received: self.frames_received,
            dropped_frames: self.dropped_frames,
            duplicate_packets: self.duplicate_packets,
            ..MediaStats::default()
        };
        stats.set_loss(self.lost_fragments, self.expected_fragments);
        stats
    }
}

struct SequenceWindow {
    highest: Option<u32>,
    seen: [u64; SEQ_WINDOW_WORDS],
}

impl Default for SequenceWindow {
    fn default() -> Self {
        Self {
            highest: None,
            seen: [0; SEQ_WINDOW_WORDS],
        }
    }
}

impl SequenceWindow {
    fn observe(&mut self, sequence: u32) -> bool {
        let Some(highest) = self.highest else {
            self.highest = Some(sequence);
            self.seen[0] = 1;
            return true;
        };
        if sequence > highest {
            let shift = (sequence - highest) as usize;
            if shift >= SEQ_WINDOW_BITS {
                self.seen = [0; SEQ_WINDOW_WORDS];
            } else {
                self.shift_left(shift);
            }
            self.seen[0] |= 1;
            self.highest = Some(sequence);
            return true;
        }
        let distance = (highest - sequence) as usize;
        if distance >= SEQ_WINDOW_BITS {
            return false;
        }
        let word = distance / 64;
        let bit = distance % 64;
        let mask = 1u64 << bit;
        if self.seen[word] & mask != 0 {
            return false;
        }
        self.seen[word] |= mask;
        true
    }

    fn shift_left(&mut self, shift: usize) {
        let word_shift = shift / 64;
        let bit_shift = shift % 64;
        if word_shift >= SEQ_WINDOW_WORDS {
            self.seen = [0; SEQ_WINDOW_WORDS];
            return;
        }
        if bit_shift == 0 {
            for index in (word_shift..SEQ_WINDOW_WORDS).rev() {
                self.seen[index] = self.seen[index - word_shift];
            }
        } else {
            for index in (word_shift..SEQ_WINDOW_WORDS).rev() {
                let mut value = self.seen[index - word_shift] << bit_shift;
                if index > word_shift {
                    value |= self.seen[index - word_shift - 1] >> (64 - bit_shift);
                }
                self.seen[index] = value;
            }
        }
        for index in 0..word_shift.min(SEQ_WINDOW_WORDS) {
            self.seen[index] = 0;
        }
    }
}
