use futures_util::{SinkExt, StreamExt};
use nc_core::{AudioCodec, DeviceId};
use rand::RngCore;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

pub const BRIDGE_KIND_VIDEO: u8 = 1;
pub const BRIDGE_KIND_AUDIO: u8 = 2;
pub const BRIDGE_KIND_CONTROL: u8 = 3;

pub struct MediaBridge {
    port: u16,
    token: String,
    sender: broadcast::Sender<Vec<u8>>,
    task: tokio::task::JoinHandle<()>,
}

impl MediaBridge {
    #[allow(clippy::result_large_err)]
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let token = random_token();
        let (sender, _) = broadcast::channel::<Vec<u8>>(128);
        let expected_token = token.clone();
        let broadcast_sender = sender.clone();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(error) => {
                        tracing::warn!(%error, "媒体桥接受连接失败");
                        continue;
                    }
                };
                let token = expected_token.clone();
                let mut receiver = broadcast_sender.subscribe();
                tokio::spawn(async move {
                    let callback = move |request: &Request, response: Response| {
                        if token_matches(request.uri().query(), &token) {
                            Ok(response)
                        } else {
                            let mut response =
                                ErrorResponse::new(Some("invalid media token".to_string()));
                            *response.status_mut() = StatusCode::UNAUTHORIZED;
                            Err(response)
                        }
                    };
                    let websocket = match accept_hdr_async(stream, callback).await {
                        Ok(websocket) => websocket,
                        Err(error) => {
                            tracing::debug!(%error, "媒体桥 WebSocket 握手失败");
                            return;
                        }
                    };
                    let (mut sink, mut source) = websocket.split();
                    loop {
                        tokio::select! {
                            frame = receiver.recv() => match frame {
                                Ok(bytes) => {
                                    if sink.send(Message::Binary(bytes.into())).await.is_err() {
                                        break;
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                    tracing::debug!(skipped, "媒体桥客户端落后，丢弃旧帧");
                                }
                                Err(broadcast::error::RecvError::Closed) => break,
                            },
                            incoming = source.next() => match incoming {
                                Some(Ok(Message::Close(_))) | None => break,
                                Some(Ok(Message::Ping(payload))) => {
                                    if sink.send(Message::Pong(payload)).await.is_err() {
                                        break;
                                    }
                                }
                                Some(Err(error)) => {
                                    tracing::debug!(%error, "媒体桥连接错误");
                                    break;
                                }
                                _ => {}
                            },
                        }
                    }
                });
            }
        });
        Ok(Self {
            port,
            token,
            sender,
            task,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn send_raw(&self, bytes: Vec<u8>) {
        let _ = self.sender.send(bytes);
    }

    pub fn send_video(&self, meta: &VideoFrameMeta, access_unit: &[u8]) {
        self.send_raw(video_frame(meta, access_unit));
    }

    pub fn send_audio(
        &self,
        codec: AudioCodec,
        sample_rate: u32,
        channels: u16,
        timestamp_us: u64,
        packet: &[u8],
    ) {
        self.send_raw(audio_frame(
            codec,
            sample_rate,
            channels,
            timestamp_us,
            packet,
        ));
    }

    pub fn send_control(&self, json: &serde_json::Value) {
        self.send_raw(control_frame(json));
    }
}

impl Drop for MediaBridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn token_matches(query: Option<&str>, expected: &str) -> bool {
    let Some(query) = query else {
        return false;
    };
    query.split('&').any(|pair| {
        let mut parts = pair.splitn(2, '=');
        parts.next() == Some("token") && parts.next() == Some(expected)
    })
}

#[derive(Debug, Clone, Copy)]
pub struct VideoFrameMeta {
    pub stream_id: u16,
    pub device_id: DeviceId,
    pub keyframe: bool,
    pub width: u16,
    pub height: u16,
    pub timestamp_us: u64,
}

pub fn video_frame(meta: &VideoFrameMeta, access_unit: &[u8]) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(36 + access_unit.len());
    buffer.push(BRIDGE_KIND_VIDEO);
    buffer.extend_from_slice(&meta.stream_id.to_le_bytes());
    buffer.extend_from_slice(meta.device_id.as_bytes());
    buffer.push(u8::from(meta.keyframe));
    buffer.extend_from_slice(&meta.width.to_le_bytes());
    buffer.extend_from_slice(&meta.height.to_le_bytes());
    buffer.extend_from_slice(&meta.timestamp_us.to_le_bytes());
    buffer.extend_from_slice(&(access_unit.len() as u32).to_le_bytes());
    buffer.extend_from_slice(access_unit);
    buffer
}

pub fn audio_frame(
    codec: AudioCodec,
    sample_rate: u32,
    channels: u16,
    timestamp_us: u64,
    packet: &[u8],
) -> Vec<u8> {
    let codec_tag = match codec {
        AudioCodec::Opus => 1u8,
        AudioCodec::PcmS16Le => 2u8,
    };
    let mut buffer = Vec::with_capacity(1 + 1 + 4 + 2 + 8 + 4 + packet.len());
    buffer.push(BRIDGE_KIND_AUDIO);
    buffer.push(codec_tag);
    buffer.extend_from_slice(&sample_rate.to_le_bytes());
    buffer.extend_from_slice(&channels.to_le_bytes());
    buffer.extend_from_slice(&timestamp_us.to_le_bytes());
    buffer.extend_from_slice(&(packet.len() as u32).to_le_bytes());
    buffer.extend_from_slice(packet);
    buffer
}

pub fn control_frame(json: &serde_json::Value) -> Vec<u8> {
    let text = serde_json::to_vec(json).unwrap_or_default();
    let mut buffer = Vec::with_capacity(1 + text.len());
    buffer.push(BRIDGE_KIND_CONTROL);
    buffer.extend_from_slice(&text);
    buffer
}

