use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;

use nc_core::{EncodedVideoFrame, NexusError, Result, StreamId};
use parking_lot::Mutex;
use tokio::net::UdpSocket;

use crate::config::MediaConfig;
use crate::fragment::Fragmenter;
use crate::key::MediaKey;
use crate::stats::MediaStats;
use crate::wire::{MediaPacket, NackBody};

const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;

pub struct MediaSender {
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    key: MediaKey,
    config: MediaConfig,
    fragmenter: Mutex<Fragmenter>,
    cache: Mutex<FrameCache>,
    stats: Mutex<MediaStats>,
}

impl MediaSender {
    pub fn new(socket: UdpSocket, peer: SocketAddr, key: MediaKey, config: MediaConfig) -> Self {
        Self::new_shared(Arc::new(socket), peer, key, config)
    }

    pub fn new_shared(
        socket: Arc<UdpSocket>,
        peer: SocketAddr,
        key: MediaKey,
        config: MediaConfig,
    ) -> Self {
        let cache = FrameCache::new(config.max_pending_frames.max(1));
        Self {
            socket,
            peer,
            key,
            config: config.clone(),
            fragmenter: Mutex::new(Fragmenter::new(config)),
            cache: Mutex::new(cache),
            stats: Mutex::new(MediaStats::default()),
        }
    }

    pub async fn send_video(&self, frame: &EncodedVideoFrame, stream_id: StreamId) -> Result<()> {
        let packets = {
            let mut fragmenter = self.fragmenter.lock();
            fragmenter.fragment_video(frame, stream_id)?
        };
        self.send_media_packets(packets).await
    }

    pub async fn send_audio(
        &self,
        timestamp_us: u64,
        data: impl AsRef<[u8]>,
        stream_id: StreamId,
    ) -> Result<()> {
        let packets = {
            let mut fragmenter = self.fragmenter.lock();
            fragmenter.fragment_audio(timestamp_us, data.as_ref(), stream_id)?
        };
        self.send_media_packets(packets).await
    }

    pub fn set_fec_parity(&self, parity: u8) {
        self.fragmenter.lock().set_fec_parity(parity);
    }

    pub async fn send_nack(&self, stream_id: StreamId, body: &NackBody) -> Result<()> {
        let session_id = self.fragmenter.lock().config().session_id;
        let packet = MediaPacket::from_nack(session_id, stream_id, body)?;
        let encoded = packet.encode(&self.key)?;
        self.socket.send_to(&encoded, self.peer).await?;
        let mut stats = self.stats.lock();
        stats.nack_sent += 1;
        stats.bytes_sent += encoded.len() as u64;
        Ok(())
    }

    pub async fn handle_nack(&self, stream_id: StreamId, body: &NackBody) -> Result<usize> {
        {
            let mut stats = self.stats.lock();
            stats.nack_received += 1;
        }
        let missing: BTreeSet<u16> = body.missing.iter().copied().collect();
        let packets = {
            let cache = self.cache.lock();
            cache.missing_packets(stream_id, body.frame_id, &missing)
        };
        let mut bytes_sent = 0u64;
        for packet in &packets {
            match self.socket.send_to(packet, self.peer).await {
                Ok(_) => bytes_sent += packet.len() as u64,
                Err(error) => {
                    self.stats.lock().bytes_sent += bytes_sent;
                    return Err(error.into());
                }
            }
        }
        self.stats.lock().bytes_sent += bytes_sent;
        Ok(packets.len())
    }

    pub fn stats(&self) -> MediaStats {
        self.stats.lock().clone()
    }

    pub fn set_last_rtt_ms(&self, rtt_ms: u64) {
        self.stats.lock().last_rtt_ms = rtt_ms;
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    pub fn config(&self) -> &MediaConfig {
        &self.config
    }

    async fn send_media_packets(&self, packets: Vec<MediaPacket>) -> Result<()> {
        if packets.is_empty() {
            return Err(NexusError::Internal("分片结果为空".to_string()));
        }
        let frame_id = packets[0].header.frame_id;
        let mut data_packets: Vec<Option<Vec<u8>>> = vec![None; packets.len()];
        let mut encoded_packets = Vec::with_capacity(packets.len());
        for packet in &packets {
            let encoded = packet.encode(&self.key)?;
            if !packet.header.is_parity() {
                let index = usize::from(packet.header.fragment_index);
                if let Some(slot) = data_packets.get_mut(index) {
                    *slot = Some(encoded.clone());
                }
            }
            encoded_packets.push(encoded);
        }

        let mut sent_bytes = 0u64;
        for encoded in &encoded_packets {
            match self.socket.send_to(encoded, self.peer).await {
                Ok(_) => sent_bytes += encoded.len() as u64,
                Err(error) => {
                    self.stats.lock().bytes_sent += sent_bytes;
                    return Err(error.into());
                }
            }
        }
        {
            let mut stats = self.stats.lock();
            stats.bytes_sent += sent_bytes;
            stats.frames_sent += 1;
        }

        let data_packets: Vec<Vec<u8>> = data_packets.into_iter().flatten().collect();
        if data_packets.len() == usize::from(packets[0].header.fragment_count) {
            self.cache.lock().insert(CachedFrame::new(
                packets[0].header.stream_id,
                frame_id,
                data_packets,
            ));
        }
        Ok(())
    }
}

struct CachedFrame {
    stream_id: StreamId,
    frame_id: u32,
    packets: Vec<Vec<u8>>,
    bytes: usize,
}

impl CachedFrame {
    fn new(stream_id: StreamId, frame_id: u32, packets: Vec<Vec<u8>>) -> Self {
        let bytes = packets.iter().map(Vec::len).sum();
        Self {
            stream_id,
            frame_id,
            packets,
            bytes,
        }
    }
}

struct FrameCache {
    frames: VecDeque<CachedFrame>,
    max_frames: usize,
    total_bytes: usize,
}

impl FrameCache {
    fn new(max_frames: usize) -> Self {
        Self {
            frames: VecDeque::new(),
            max_frames: max_frames.max(1),
            total_bytes: 0,
        }
    }

    fn insert(&mut self, frame: CachedFrame) {
        if frame.bytes > MAX_CACHE_BYTES {
            return;
        }
        while self.frames.len() >= self.max_frames
            || self.total_bytes.saturating_add(frame.bytes) > MAX_CACHE_BYTES
        {
            let Some(evicted) = self.frames.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.bytes);
        }
        self.total_bytes = self.total_bytes.saturating_add(frame.bytes);
        self.frames.push_back(frame);
    }

    fn missing_packets(
        &self,
        stream_id: StreamId,
        frame_id: u32,
        missing: &BTreeSet<u16>,
    ) -> Vec<Vec<u8>> {
        let Some(frame) = self
            .frames
            .iter()
            .rev()
            .find(|frame| frame.stream_id == stream_id && frame.frame_id == frame_id)
        else {
            return Vec::new();
        };
        missing
            .iter()
            .filter_map(|index| frame.packets.get(usize::from(*index)).cloned())
            .collect()
    }
}
