use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nc_core::StreamId;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::debug;

use crate::config::MediaConfig;
use crate::key::MediaKey;
use crate::reassembly::{AssembledFrame, Reassembler, ReassemblyEvent};
use crate::stats::MediaStats;
use crate::wire::{MediaPacket, NackBody, PacketKind};

const RECV_BUFFER_BYTES: usize = 65_535;
const TICK_MS: u64 = 5;
const STATS_EVERY_TICKS: u32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub enum MediaEvent {
    Frame(AssembledFrame),
    Nack { stream_id: StreamId, body: NackBody },
    Stats(MediaStats),
}

pub struct MediaReceiver {
    socket: Arc<UdpSocket>,
    key: MediaKey,
    config: MediaConfig,
    reassembler: Reassembler,
    peer: Option<SocketAddr>,
    bytes_received: u64,
    nack_sent: u64,
    nack_received: u64,
    last_rtt_ms: u64,
}

impl MediaReceiver {
    pub fn new(socket: UdpSocket, key: MediaKey, config: MediaConfig) -> Self {
        Self::new_shared(Arc::new(socket), key, config)
    }

    pub fn new_shared(socket: Arc<UdpSocket>, key: MediaKey, config: MediaConfig) -> Self {
        Self {
            socket,
            key,
            reassembler: Reassembler::new(config.clone()),
            config,
            peer: None,
            bytes_received: 0,
            nack_sent: 0,
            nack_received: 0,
            last_rtt_ms: 0,
        }
    }

    pub fn config(&self) -> &MediaConfig {
        &self.config
    }

    pub fn set_last_rtt_ms(&mut self, rtt_ms: u64) {
        self.last_rtt_ms = rtt_ms;
    }

    pub fn stats(&self) -> MediaStats {
        let mut stats = self.reassembler.stats();
        stats.bytes_received = self.bytes_received;
        stats.nack_sent = self.nack_sent;
        stats.nack_received = self.nack_received;
        stats.last_rtt_ms = self.last_rtt_ms;
        stats
    }

    pub async fn run(mut self, event_tx: mpsc::Sender<MediaEvent>) {
        let mut buffer = vec![0u8; RECV_BUFFER_BYTES];
        let mut ticker = tokio::time::interval(Duration::from_millis(TICK_MS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut ticks = 0u32;
        loop {
            tokio::select! {
                received = self.socket.recv_from(&mut buffer) => {
                    match received {
                        Ok((size, address)) => {
                            self.bytes_received = self
                                .bytes_received
                                .saturating_add(size as u64);
                            match MediaPacket::decode(&buffer[..size], &self.key) {
                                Ok(packet) => {
                                    if !self.handle_packet(packet, address, &event_tx).await {
                                        return;
                                    }
                                }
                                Err(error) => {
                                    debug!(%error, "丢弃无法解密的媒体包");
                                }
                            }
                        }
                        Err(error) => {
                            debug!(%error, "媒体 UDP 接收失败");
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    }
                }
                _ = ticker.tick() => {
                    let now = Instant::now();
                    if !self.flush(now, &event_tx).await {
                        return;
                    }
                    ticks = ticks.wrapping_add(1);
                    if ticks % STATS_EVERY_TICKS == 0
                        && event_tx.send(MediaEvent::Stats(self.stats())).await.is_err()
                    {
                        return;
                    }
                }
            }
        }
    }

    async fn handle_packet(
        &mut self,
        packet: MediaPacket,
        from: SocketAddr,
        event_tx: &mpsc::Sender<MediaEvent>,
    ) -> bool {
        if packet.header.session_id != self.config.session_id {
            return true;
        }
        self.peer = Some(from);
        if packet.header.kind == PacketKind::Nack {
            self.nack_received = self.nack_received.saturating_add(1);
            match packet.nack_body() {
                Ok(body) => event_tx
                    .send(MediaEvent::Nack {
                        stream_id: packet.header.stream_id,
                        body,
                    })
                    .await
                    .is_ok(),
                Err(error) => {
                    debug!(%error, "NACK 载荷解析失败");
                    true
                }
            }
        } else {
            let now = Instant::now();
            let event = self.reassembler.push(packet, now);
            if let ReassemblyEvent::Complete(frame) = event {
                if event_tx.send(MediaEvent::Frame(frame)).await.is_err() {
                    return false;
                }
            } else if let ReassemblyEvent::Rejected(message) = event {
                debug!(%message, "媒体包被重组器拒绝");
            }
            self.flush(now, event_tx).await
        }
    }

    async fn flush(&mut self, now: Instant, event_tx: &mpsc::Sender<MediaEvent>) -> bool {
        for event in self.reassembler.poll(now) {
            if let ReassemblyEvent::Complete(frame) = event {
                if event_tx.send(MediaEvent::Frame(frame)).await.is_err() {
                    return false;
                }
            }
        }
        for request in self.reassembler.take_nacks() {
            let body = NackBody {
                frame_id: request.frame_id,
                missing: request.missing,
            };
            let Some(peer) = self.peer else {
                continue;
            };
            let encoded = match MediaPacket::from_nack(
                self.config.session_id,
                request.stream_id,
                &body,
            )
            .and_then(|packet| packet.encode(&self.key))
            {
                Ok(encoded) => encoded,
                Err(error) => {
                    debug!(%error, "构造 NACK 失败");
                    continue;
                }
            };
            match self.socket.send_to(&encoded, peer).await {
                Ok(_) => {
                    self.nack_sent = self.nack_sent.saturating_add(1);
                }
                Err(error) => {
                    debug!(%error, "发送 NACK 失败");
                }
            }
        }
        true
    }
}
