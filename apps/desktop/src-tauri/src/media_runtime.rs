use crate::runtime::{
    AppEvent, AudioBinding, MediaRuntime, OutgoingAudio, OutgoingStream, StreamBinding,
};
use nc_capture::{
    create_audio_capturer, create_screen_capturer, AudioCaptureConfig, CapturedFrame,
    ScreenCaptureConfig,
};
use nc_codec::{create_audio_encoder, EncoderConfig, EncoderPreset, VideoEncoder};
use nc_core::{AudioCodec, DeviceId, EncodedVideoFrame, Result, StreamId, VideoTarget};
use nc_media::{MediaEvent, MediaKey, MediaReceiver, MediaSender};
use nc_observability::MetricsRegistry;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy)]
pub struct AudioStreamDescriptor {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct OutgoingRequest {
    pub stream_id: StreamId,
    pub device_id: DeviceId,
    pub monitor_id: u32,
    pub target: VideoTarget,
    pub with_audio: bool,
    pub sender: Arc<MediaSender>,
    pub metrics: Arc<MetricsRegistry>,
    pub excluded_window_id: Option<u32>,
    pub encoder_vendor: Option<nc_codec::EncoderVendor>,
    pub prefer_speed: bool,
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

impl MediaRuntime {
    pub fn start_receiver(
        &self,
        connection_id: u64,
        media_key: [u8; 32],
        session_id: u64,
        events: UnboundedSender<AppEvent>,
    ) {
        self.clear_session();
        self.set_session_id(session_id);
        let mut config = self.config.clone();
        config.session_id = session_id;
        let receiver =
            MediaReceiver::new_shared(self.recv_socket.clone(), MediaKey::new(media_key), config);
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
        let task = tokio::spawn(receiver.run(event_tx));
        *self.receiver_task.lock() = Some(task);
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if events
                    .send(AppEvent::Media {
                        connection_id,
                        event,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    pub fn start_sender(
        &self,
        peer: std::net::SocketAddr,
        media_key: [u8; 32],
        session_id: u64,
    ) -> Arc<MediaSender> {
        let mut config = self.config.clone();
        config.fec_parity = self.fec_parity();
        config.session_id = session_id;
        let sender = Arc::new(MediaSender::new_shared(
            self.recv_socket.clone(),
            peer,
            MediaKey::new(media_key),
            config,
        ));
        *self.sender.write() = Some(sender.clone());
        *self.peer_addr.write() = Some(peer);
        sender
    }

    pub async fn start_outgoing(
        &self,
        request: OutgoingRequest,
        _events: UnboundedSender<AppEvent>,
    ) -> Result<Option<AudioStreamDescriptor>> {
        let existing = { self.outgoing.lock().remove(&request.stream_id) };
        if let Some(mut existing) = existing {
            let _ = tokio::task::spawn_blocking(move || existing.stop_blocking()).await;
        }

        let encoder_config = EncoderConfig {
            width: request.target.width,
            height: request.target.height,
            fps: request.target.fps,
            bitrate_kbps: request.target.bitrate_kbps,
            keyframe_interval: request.target.fps.saturating_mul(2).max(1),
            preset: EncoderPreset::Fast,
        };
        encoder_config.validate()?;
        let encoder_vendor = request.encoder_vendor;
        let prefer_speed = request.prefer_speed;

        let mut capturer = create_screen_capturer()?;
        let (frame_tx, frame_rx) = crossbeam_channel::bounded(2);
        capturer.start(
            ScreenCaptureConfig {
                monitor_id: request.monitor_id,
                fps: request.target.fps,
                include_cursor: false,
                target_width: request.target.width,
                target_height: request.target.height,
                excluded_window_id: request.excluded_window_id,
                prefer_gpu_pixels: !prefer_speed,
            },
            frame_tx,
        )?;

        let stop = Arc::new(AtomicBool::new(false));
        let keyframe_requested = Arc::new(AtomicBool::new(false));

        let (encoded_tx, mut encoded_rx) =
            tokio::sync::mpsc::unbounded_channel::<EncodedVideoFrame>();
        let encoder_stop = stop.clone();
        let encoder_keyframe = keyframe_requested.clone();
        let encoder_metrics = request.metrics.clone();
        let encoder_task = tokio::task::spawn_blocking(move || {
            let mut encoder: Option<EncoderSlot> = None;
            let mut last_encode_us = 0u64;
            while !encoder_stop.load(Ordering::Relaxed) {
                let captured = match frame_rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(first) => Some(drain_to_latest(first, &frame_rx)),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                };

                if encoder.is_none() {
                    let Some(captured) = captured.as_ref() else {
                        continue;
                    };
                    match create_encoder_slot(
                        captured,
                        encoder_config,
                        encoder_vendor,
                        prefer_speed,
                    ) {
                        Ok(slot) => encoder = Some(slot),
                        Err(error) => {
                            #[cfg(windows)]
                            if captured.gpu.is_some() {
                                tracing::error!(
                                    %error,
                                    "GPU 纹理编码器初始化失败，本进程禁用 GPU 采集"
                                );
                                nc_capture::disable_gpu_capture();
                                continue;
                            }
                            #[cfg(target_os = "macos")]
                            if captured.pixel_buffer.is_some() {
                                continue;
                            }
                            tracing::error!(%error, "初始化视频编码器失败，停止视频发送");
                            break;
                        }
                    }
                }
                let encoder = encoder.as_mut().expect("编码器已初始化");
                if encoder_keyframe.swap(false, Ordering::SeqCst) {
                    encoder.request_keyframe();
                }

                if let Some(captured) = captured.as_ref() {
                    encoder_metrics.record_capture_frame();
                    let capture_us = now_us().saturating_sub(captured.captured_at_us);
                    let started = Instant::now();
                    let result = encoder.encode(captured);
                    last_encode_us = started.elapsed().as_micros() as u64;
                    match result {
                        Ok(Some(encoded)) => {
                            encoder_metrics.record_video_frame(
                                encoded.data.len() as u64,
                                last_encode_us,
                                capture_us,
                            );
                            if encoded_tx.send(encoded).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(%error, "H.264 编码失败，丢弃该帧");
                        }
                    }
                }

                match encoder.poll() {
                    Ok(outputs) => {
                        for encoded in outputs {
                            let capture_us = now_us().saturating_sub(encoded.timestamp_us);
                            encoder_metrics.record_video_frame(
                                encoded.data.len() as u64,
                                last_encode_us,
                                capture_us,
                            );
                            if encoded_tx.send(encoded).is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "读取编码器输出失败，停止视频发送");
                        break;
                    }
                }
            }
        });

        let sender = request.sender.clone();
        let stream_id = request.stream_id;
        let send_task = tokio::spawn(async move {
            while let Some(frame) = encoded_rx.recv().await {
                if let Err(error) = sender.send_video(&frame, stream_id).await {
                    tracing::warn!(%error, "发送视频帧失败");
                    break;
                }
            }
        });

        let audio = if request.with_audio {
            match self.start_audio(request.sender.clone()) {
                Ok((audio, descriptor)) => Some((audio, descriptor)),
                Err(error) => {
                    tracing::warn!(%error, "系统音频采集不可用，继续仅视频");
                    None
                }
            }
        } else {
            None
        };

        let (audio_stream, descriptor) = match audio {
            Some((audio, descriptor)) => (Some(audio), Some(descriptor)),
            None => (None, None),
        };

        let outgoing = OutgoingStream {
            stream_id: request.stream_id,
            device_id: request.device_id,
            monitor_id: request.monitor_id,
            stop,
            keyframe_requested,
            capture: Some(capturer),
            audio: audio_stream,
            encoder_task: Some(encoder_task),
            send_task: Some(send_task),
            capture_forwarder: None,
        };
        self.outgoing.lock().insert(request.stream_id, outgoing);
        Ok(descriptor)
    }

    fn start_audio(
        &self,
        sender: Arc<MediaSender>,
    ) -> Result<(OutgoingAudio, AudioStreamDescriptor)> {
        let (audio_encoder, codec) = create_audio_encoder(48_000, 2, 64_000);
        let descriptor = AudioStreamDescriptor {
            codec,
            sample_rate: 48_000,
            channels: 2,
        };
        let (packet_tx, mut packet_rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Vec<u8>)>();
        let mut capturer = create_audio_capturer()?;
        let (frame_tx, frame_rx) = crossbeam_channel::bounded(4);
        capturer.start(AudioCaptureConfig::default(), frame_tx)?;
        let stop = Arc::new(AtomicBool::new(false));
        let encoder_stop = stop.clone();
        let mut audio_encoder = audio_encoder;
        let forwarder = std::thread::spawn(move || {
            while let Ok(frame) = frame_rx.recv() {
                if encoder_stop.load(Ordering::Relaxed) {
                    break;
                }
                match audio_encoder.encode(&frame) {
                    Ok(packet) => {
                        if packet_tx.send((frame.timestamp_us, packet)).is_err() {
                            break;
                        }
                    }
                    Err(error) => tracing::warn!(%error, "音频编码失败"),
                }
            }
        });
        let send_task = tokio::spawn(async move {
            while let Some((timestamp_us, packet)) = packet_rx.recv().await {
                if let Err(error) = sender
                    .send_audio(timestamp_us, &packet, StreamId::AUDIO)
                    .await
                {
                    tracing::warn!(%error, "发送音频包失败");
                    break;
                }
            }
        });
        Ok((
            OutgoingAudio {
                capturer,
                stop,
                send_task: Some(send_task),
                forwarder: Some(forwarder),
            },
            descriptor,
        ))
    }

    pub async fn stop_outgoing(&self, stream_id: StreamId) {
        let stream = { self.outgoing.lock().remove(&stream_id) };
        if let Some(mut stream) = stream {
            let _ = tokio::task::spawn_blocking(move || stream.stop_blocking()).await;
        }
    }

    pub fn request_keyframe(&self, stream_id: StreamId) {
        if let Some(stream) = self.outgoing.lock().get(&stream_id) {
            stream.keyframe_requested.store(true, Ordering::SeqCst);
        }
    }

    pub fn request_all_keyframes(&self) {
        for stream in self.outgoing.lock().values() {
            stream.keyframe_requested.store(true, Ordering::SeqCst);
        }
    }

    pub fn bind_video(
        &self,
        stream_id: StreamId,
        device_id: DeviceId,
        width: u32,
        height: u32,
        fps: u32,
    ) {
        let audio = self
            .bindings
            .read()
            .get(&stream_id)
            .and_then(|binding| binding.audio);
        self.bindings.write().insert(
            stream_id,
            StreamBinding {
                device_id,
                width,
                height,
                fps,
                audio,
            },
        );
    }

    pub fn bind_audio(&self, stream_id: StreamId, device_id: DeviceId, audio: AudioBinding) {
        let mut bindings = self.bindings.write();
        let entry = bindings.entry(stream_id).or_insert(StreamBinding {
            device_id,
            width: 0,
            height: 0,
            fps: 0,
            audio: None,
        });
        entry.device_id = device_id;
        entry.audio = Some(audio);
    }

    pub async fn handle_event(&self, event: MediaEvent, bridge: &crate::media_bridge::MediaBridge) {
        match event {
            MediaEvent::Frame(frame) => {
                let Some(binding) = self.binding(frame.stream_id) else {
                    return;
                };
                if frame.is_audio {
                    if let Some(audio) = binding.audio {
                        bridge.send_audio(
                            audio.codec,
                            audio.sample_rate,
                            audio.channels,
                            frame.timestamp_us,
                            &frame.data,
                        );
                    }
                } else {
                    bridge.send_video(
                        &crate::media_bridge::VideoFrameMeta {
                            stream_id: frame.stream_id.0,
                            device_id: binding.device_id,
                            keyframe: frame.keyframe,
                            width: binding.width.min(u16::MAX as u32) as u16,
                            height: binding.height.min(u16::MAX as u32) as u16,
                            timestamp_us: frame.timestamp_us,
                        },
                        &frame.data,
                    );
                }
            }
            MediaEvent::Nack { stream_id, body } => {
                if let Some(sender) = self.sender() {
                    if let Err(error) = sender.handle_nack(stream_id, &body).await {
                        tracing::debug!(%error, "NACK 重传失败");
                    }
                }
            }
            MediaEvent::Stats(stats) => {
                tracing::trace!(
                    frames = stats.frames_received,
                    loss = stats.loss_percent,
                    "媒体接收统计"
                );
            }
        }
    }
}

enum EncoderSlot {
    Cpu(Box<dyn nc_codec::VideoEncoder>),
    #[cfg(windows)]
    Gpu(Box<nc_codec::MfH264Encoder>),
    #[cfg(target_os = "macos")]
    Vt(Box<nc_codec::VtH264Encoder>),
}

impl EncoderSlot {
    fn request_keyframe(&mut self) {
        match self {
            Self::Cpu(encoder) => encoder.request_keyframe(),
            #[cfg(windows)]
            Self::Gpu(encoder) => encoder.request_keyframe(),
            #[cfg(target_os = "macos")]
            Self::Vt(encoder) => encoder.request_keyframe(),
        }
    }

    fn encode(&mut self, captured: &CapturedFrame) -> Result<Option<EncodedVideoFrame>> {
        match self {
            Self::Cpu(encoder) => encoder.encode(&captured.frame),
            #[cfg(windows)]
            Self::Gpu(encoder) => {
                let gpu = captured.gpu.as_ref().ok_or_else(|| {
                    nc_core::NexusError::Media("GPU 编码器收到 CPU 帧".to_string())
                })?;
                encoder.encode_texture(gpu.texture(), captured.captured_at_us)
            }
            #[cfg(target_os = "macos")]
            Self::Vt(encoder) => {
                let pixel_buffer = captured.pixel_buffer.as_ref().ok_or_else(|| {
                    nc_core::NexusError::Media("VideoToolbox 编码器收到 CPU 帧".to_string())
                })?;
                encoder.encode_pixel_buffer(&pixel_buffer.pixel_buffer, captured.captured_at_us)
            }
        }
    }

    fn poll(&mut self) -> Result<Vec<EncodedVideoFrame>> {
        match self {
            Self::Cpu(encoder) => encoder.poll(),
            #[cfg(windows)]
            Self::Gpu(encoder) => encoder.poll(),
            #[cfg(target_os = "macos")]
            Self::Vt(encoder) => encoder.poll(),
        }
    }
}

#[cfg_attr(not(windows), allow(unused_variables))]
fn create_encoder_slot(
    captured: &CapturedFrame,
    config: EncoderConfig,
    vendor: Option<nc_codec::EncoderVendor>,
    prefer_speed: bool,
) -> Result<EncoderSlot> {
    #[cfg(windows)]
    if let Some(gpu) = captured.gpu.as_ref() {
        let encoder = nc_codec::MfH264Encoder::new_with_device(config, vendor, gpu.device())?;
        tracing::info!(
            backend = encoder.backend_name(),
            width = gpu.width(),
            height = gpu.height(),
            "使用 GPU 纹理零拷贝编码"
        );
        return Ok(EncoderSlot::Gpu(Box::new(encoder)));
    }
    #[cfg(target_os = "macos")]
    if captured.pixel_buffer.is_some() {
        match nc_codec::VtH264Encoder::new(config) {
            Ok(encoder) => {
                tracing::info!(
                    backend = encoder.backend_name(),
                    width = config.width,
                    height = config.height,
                    "使用 VideoToolbox 420v 像素缓冲零拷贝编码"
                );
                return Ok(EncoderSlot::Vt(Box::new(encoder)));
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    "VideoToolbox 420v 编码器创建失败，下一帧回退 BGRA 采集"
                );
                nc_capture::disable_pixel_buffer_capture();
                return Err(error);
            }
        }
    }
    let encoder = nc_codec::create_video_encoder_tuned(config, vendor, prefer_speed)?;
    tracing::info!(
        backend = encoder.backend_name(),
        hardware = encoder.is_hardware(),
        prefer_speed,
        "使用视频编码器"
    );
    Ok(EncoderSlot::Cpu(encoder))
}

fn drain_to_latest<T>(first: T, receiver: &crossbeam_channel::Receiver<T>) -> T {
    let mut latest = first;
    while let Ok(next) = receiver.try_recv() {
        latest = next;
    }
    latest
}
