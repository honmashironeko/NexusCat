use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Sender, TrySendError};
use nc_core::{AudioFrame, NexusError, Result};
use tracing::{debug, warn};
use wasapi::{
    initialize_mta, AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, SampleType,
    StreamMode, WasapiError, WaveFormat,
};

use crate::audio_convert::{convert_to_target, PcmSampleFormat};
use crate::worker::CaptureWorker;
use crate::{AudioCaptureConfig, AudioCapturer, STARTUP_TIMEOUT};

const BUFFER_DURATION_HNS: i64 = 200_000;
const EVENT_WAIT_MS: u32 = 100;
const MAX_FRAME_MS: u32 = 1_000;
const MAX_SAMPLE_RATE: u32 = 384_000;

pub(crate) struct WasapiAudioCapturer {
    worker: CaptureWorker,
}

impl WasapiAudioCapturer {
    pub(crate) fn new() -> Self {
        Self {
            worker: CaptureWorker::new(),
        }
    }
}

impl AudioCapturer for WasapiAudioCapturer {
    fn start(&mut self, config: AudioCaptureConfig, sink: Sender<AudioFrame>) -> Result<()> {
        validate_config(&config)?;
        self.worker.spawn(
            "nc-capture-wasapi",
            STARTUP_TIMEOUT,
            move |running, startup| match LoopbackSession::start() {
                Ok(mut session) => {
                    let _ = startup.send(Ok(()));
                    session.capture_loop(&config, &sink, &running);
                }
                Err(error) => {
                    let _ = startup.send(Err(error.to_string()));
                }
            },
        )
    }

    fn stop(&mut self) {
        self.worker.stop();
    }

    fn is_running(&self) -> bool {
        self.worker.is_running()
    }
}

fn validate_config(config: &AudioCaptureConfig) -> Result<()> {
    if config.sample_rate == 0 || config.sample_rate > MAX_SAMPLE_RATE {
        return Err(NexusError::invalid(format!(
            "音频采样率必须在 1..={MAX_SAMPLE_RATE} Hz"
        )));
    }
    if config.channels != 1 && config.channels != 2 {
        return Err(NexusError::invalid("音频采集仅支持 1 或 2 个输出声道"));
    }
    if config.frame_ms == 0 || config.frame_ms > MAX_FRAME_MS {
        return Err(NexusError::invalid(format!(
            "音频帧长必须在 1..={MAX_FRAME_MS} ms"
        )));
    }
    Ok(())
}

struct SourceFormat {
    sample: PcmSampleFormat,
    rate: u32,
    channels: u16,
    block_align: usize,
}

impl SourceFormat {
    fn from_wave_format(format: &WaveFormat) -> Result<Self> {
        let bits = format.get_bitspersample();
        let sample_type = format
            .get_subformat()
            .map_err(|error| NexusError::Media(format!("无法识别音频设备子格式：{error}")))?;
        let sample = match (sample_type, bits) {
            (SampleType::Float, 32) => PcmSampleFormat::F32,
            (SampleType::Int, 8) => PcmSampleFormat::U8,
            (SampleType::Int, 16) => PcmSampleFormat::I16,
            (SampleType::Int, 24) => PcmSampleFormat::I24,
            (SampleType::Int, 32) => PcmSampleFormat::I32,
            _ => {
                return Err(NexusError::Media(format!(
                    "不支持的音频设备格式：{sample_type} {bits}-bit"
                )));
            }
        };

        let channels = format.get_nchannels();
        let rate = format.get_samplespersec();
        let block_align = format.get_blockalign() as usize;
        if channels == 0 || rate == 0 {
            return Err(NexusError::Media(format!(
                "音频设备格式无效：{rate} Hz，{channels} 声道"
            )));
        }
        let expected_block_align = usize::from(channels) * sample.bytes_per_sample();
        if block_align != expected_block_align {
            return Err(NexusError::Media(format!(
                "音频设备块对齐不匹配：{block_align} != {expected_block_align}"
            )));
        }

        Ok(Self {
            sample,
            rate,
            channels,
            block_align,
        })
    }
}

struct LoopbackSession {
    audio_client: AudioClient,
    capture_client: AudioCaptureClient,
    event: wasapi::Handle,
    source: SourceFormat,
}

impl LoopbackSession {
    fn start() -> Result<Self> {
        initialize_mta()
            .ok()
            .map_err(|error| wasapi_error("初始化 COM MTA", error))?;

        let enumerator =
            DeviceEnumerator::new().map_err(|error| wasapi_error("创建设备枚举器", error))?;
        let device = enumerator
            .get_default_device(&Direction::Render)
            .map_err(|error| wasapi_error("获取默认音频输出设备", error))?;
        let mut audio_client = device
            .get_iaudioclient()
            .map_err(|error| wasapi_error("激活音频客户端", error))?;
        let format = audio_client
            .get_mixformat()
            .map_err(|error| wasapi_error("读取设备混音格式", error))?;
        let source = SourceFormat::from_wave_format(&format)?;

        audio_client
            .initialize_client(
                &format,
                &Direction::Capture,
                &StreamMode::EventsShared {
                    autoconvert: true,
                    buffer_duration_hns: BUFFER_DURATION_HNS,
                },
            )
            .map_err(|error| wasapi_error("初始化回环音频流", error))?;

        let event = audio_client
            .set_get_eventhandle()
            .map_err(|error| wasapi_error("创建音频事件句柄", error))?;
        let capture_client = audio_client
            .get_audiocaptureclient()
            .map_err(|error| wasapi_error("创建音频采集客户端", error))?;
        audio_client
            .start_stream()
            .map_err(|error| wasapi_error("启动音频流", error))?;

        Ok(Self {
            audio_client,
            capture_client,
            event,
            source,
        })
    }

    fn capture_loop(
        &mut self,
        config: &AudioCaptureConfig,
        sink: &Sender<AudioFrame>,
        running: &AtomicBool,
    ) {
        let chunk_frames = chunk_frame_count(config.sample_rate, config.frame_ms);
        let chunk_samples = chunk_frames * usize::from(config.channels);
        let mut pending: Vec<i16> = Vec::with_capacity(chunk_samples * 2);
        let mut raw: Vec<u8> = Vec::new();
        let start_timestamp_us = crate::unix_time_us();
        let mut sent_frames: u64 = 0;

        while running.load(Ordering::SeqCst) {
            if let Err(error) = self.drain_packets(&mut raw, &mut pending, config) {
                warn!(%error, "读取音频回环数据失败，停止采集");
                break;
            }

            while pending.len() >= chunk_samples {
                let samples: Vec<i16> = pending.drain(..chunk_samples).collect();
                let timestamp_us =
                    start_timestamp_us + sent_frames * 1_000_000 / u64::from(config.sample_rate);
                sent_frames += chunk_frames as u64;
                let frame = AudioFrame {
                    sample_rate: config.sample_rate,
                    channels: config.channels,
                    timestamp_us,
                    samples,
                };
                match sink.try_send(frame) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        debug!("音频帧接收端拥塞，丢弃一帧");
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        debug!("音频帧接收端已关闭，停止采集");
                        return;
                    }
                }
            }

            if !running.load(Ordering::SeqCst) {
                break;
            }
            match self.event.wait_for_event(EVENT_WAIT_MS) {
                Ok(()) | Err(WasapiError::EventTimeout) => {}
                Err(error) => {
                    warn!(%error, "等待音频事件失败，停止采集");
                    break;
                }
            }
        }
    }

    fn drain_packets(
        &mut self,
        raw: &mut Vec<u8>,
        pending: &mut Vec<i16>,
        config: &AudioCaptureConfig,
    ) -> Result<()> {
        loop {
            let packet_frames = self
                .capture_client
                .get_next_packet_size()
                .map_err(|error| wasapi_error("查询音频包大小", error))?
                .unwrap_or(0);
            if packet_frames == 0 {
                return Ok(());
            }

            let needed = packet_frames as usize * self.source.block_align;
            if raw.len() < needed {
                raw.resize(needed, 0);
            }
            let (frames_read, buffer_info) = self
                .capture_client
                .read_from_device(&mut raw[..needed])
                .map_err(|error| wasapi_error("读取音频包", error))?;
            if frames_read == 0 {
                return Ok(());
            }

            let byte_len = frames_read as usize * self.source.block_align;
            if buffer_info.flags.silent {
                raw[..byte_len].fill(0);
            }
            let converted = convert_to_target(
                &raw[..byte_len],
                self.source.sample,
                self.source.rate,
                self.source.channels,
                config.sample_rate,
                config.channels,
            );
            pending.extend_from_slice(&converted);
        }
    }
}

impl Drop for LoopbackSession {
    fn drop(&mut self) {
        if let Err(error) = self.audio_client.stop_stream() {
            debug!(%error, "停止 WASAPI 音频流失败");
        }
    }
}

fn wasapi_error(action: &str, error: impl std::fmt::Display) -> NexusError {
    NexusError::platform(format!("WASAPI {action}失败：{error}"))
}

fn chunk_frame_count(sample_rate: u32, frame_ms: u32) -> usize {
    ((u64::from(sample_rate) * u64::from(frame_ms)) / 1_000).max(1) as usize
}
