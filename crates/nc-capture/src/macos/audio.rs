use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Sender, TrySendError};
use nc_core::{AudioFrame, NexusError, Result};
use screencapturekit::cm::{AudioBufferList, CMSampleBuffer};
use screencapturekit::prelude::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutputTrait,
    SCStreamOutputType,
};
use tracing::{debug, warn};

use crate::audio_convert::{
    convert_to_target, decode_to_i16, map_channels, resample_linear, PcmSampleFormat,
};
use crate::worker::CaptureWorker;
use crate::{AudioCaptureConfig, AudioCapturer, STARTUP_TIMEOUT};

const MAX_SAMPLE_RATE: u32 = 384_000;
const MAX_FRAME_MS: u32 = 1_000;
const SCK_SAMPLE_RATE: u32 = 48_000;
const MAX_PENDING_SECONDS: usize = 2;

pub(crate) struct SckAudioCapturer {
    worker: CaptureWorker,
}

impl SckAudioCapturer {
    pub(crate) fn new() -> Self {
        Self {
            worker: CaptureWorker::new(),
        }
    }
}

impl AudioCapturer for SckAudioCapturer {
    fn start(&mut self, config: AudioCaptureConfig, sink: Sender<AudioFrame>) -> Result<()> {
        validate_config(&config)?;
        self.worker.spawn(
            "nc-capture-sck-audio",
            STARTUP_TIMEOUT,
            move |running, startup| {
                let pending = Arc::new(Mutex::new(Vec::<i16>::new()));
                let failed = Arc::new(AtomicBool::new(false));
                match start_stream(&config, Arc::clone(&pending), Arc::clone(&failed)) {
                    Ok(stream) => {
                        let _ = startup.send(Ok(()));
                        capture_loop(&config, &sink, &running, &pending, &failed);
                        if let Err(error) = stream.stop_capture() {
                            debug!(%error, "停止 ScreenCaptureKit 音频流失败");
                        }
                    }
                    Err(error) => {
                        let _ = startup.send(Err(error.to_string()));
                    }
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

fn start_stream(
    config: &AudioCaptureConfig,
    pending: Arc<Mutex<Vec<i16>>>,
    failed: Arc<AtomicBool>,
) -> Result<SCStream> {
    let content = SCShareableContent::get().map_err(|error| {
        NexusError::platform(format!("获取 ScreenCaptureKit 内容失败：{error}"))
    })?;
    let display = content
        .displays()
        .into_iter()
        .next()
        .ok_or_else(|| NexusError::platform("ScreenCaptureKit 没有可用的显示器"))?;
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .build()
        .map_err(|error| NexusError::platform(format!("创建 SCK 音频过滤器失败：{error}")))?;

    let stream_config = SCStreamConfiguration::new()
        .with_width(16)
        .with_height(16)
        .with_captures_audio(true)
        .with_sample_rate(SCK_SAMPLE_RATE as i32)
        .with_channel_count(config.channels)
        .with_excludes_current_process_audio(true)
        .with_queue_depth(10);

    let mut stream = SCStream::new(&filter, &stream_config).map_err(|error| {
        NexusError::platform(format!("创建 ScreenCaptureKit 音频流失败：{error}"))
    })?;
    stream
        .add_output_handler(
            AudioHandler {
                pending,
                target_rate: config.sample_rate,
                target_channels: config.channels,
                failed,
            },
            SCStreamOutputType::Audio,
        )
        .map_err(|error| NexusError::platform(format!("注册 SCK 音频输出失败：{error}")))?;
    stream.start_capture().map_err(|error| {
        NexusError::platform(format!("启动 ScreenCaptureKit 音频流失败：{error}"))
    })?;
    Ok(stream)
}

fn capture_loop(
    config: &AudioCaptureConfig,
    sink: &Sender<AudioFrame>,
    running: &AtomicBool,
    pending: &Mutex<Vec<i16>>,
    failed: &AtomicBool,
) {
    let chunk_frames = chunk_frame_count(config.sample_rate, config.frame_ms);
    let chunk_samples = chunk_frames * usize::from(config.channels);
    let start_timestamp_us = crate::unix_time_us();
    let mut sent_frames: u64 = 0;

    while running.load(Ordering::SeqCst) {
        if failed.load(Ordering::Acquire) {
            warn!("ScreenCaptureKit 音频处理失败，停止采集");
            break;
        }
        let mut buffered = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while buffered.len() >= chunk_samples {
            let samples: Vec<i16> = buffered.drain(..chunk_samples).collect();
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
                Err(TrySendError::Full(_)) => debug!("音频帧接收端拥塞，丢弃一帧"),
                Err(TrySendError::Disconnected(_)) => {
                    debug!("音频帧接收端已关闭，停止采集");
                    return;
                }
            }
        }
        drop(buffered);
        thread::sleep(Duration::from_millis(5));
    }
}

struct AudioHandler {
    pending: Arc<Mutex<Vec<i16>>>,
    target_rate: u32,
    target_channels: u16,
    failed: Arc<AtomicBool>,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        if output_type != SCStreamOutputType::Audio {
            return;
        }
        match convert_sample(&sample, self.target_rate, self.target_channels) {
            Ok(converted) => {
                if converted.is_empty() {
                    return;
                }
                let mut pending = self
                    .pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                pending.extend_from_slice(&converted);
                let max_samples = self.target_rate as usize
                    * usize::from(self.target_channels)
                    * MAX_PENDING_SECONDS;
                if pending.len() > max_samples {
                    let excess = pending.len() - max_samples;
                    pending.drain(..excess);
                }
            }
            Err(error) => {
                warn!(%error, "ScreenCaptureKit 音频缓冲转换失败");
                self.failed.store(true, Ordering::Release);
            }
        }
    }
}

fn convert_sample(
    sample: &CMSampleBuffer,
    target_rate: u32,
    target_channels: u16,
) -> Result<Vec<i16>> {
    let format = sample.format_description();
    let source_rate = format
        .as_ref()
        .and_then(|format| format.audio_sample_rate())
        .and_then(|rate| u32::try_from(rate.round() as i64).ok())
        .filter(|rate| *rate > 0)
        .unwrap_or(SCK_SAMPLE_RATE);
    let source_channels = format
        .as_ref()
        .and_then(|format| format.audio_channel_count())
        .and_then(|channels| u16::try_from(channels).ok())
        .filter(|channels| *channels > 0)
        .unwrap_or(target_channels);
    let bits = format
        .as_ref()
        .and_then(|format| format.audio_bits_per_channel())
        .unwrap_or(32);
    let is_float = format
        .as_ref()
        .map(|format| format.audio_is_float())
        .unwrap_or(bits == 32);
    let sample_format = match (is_float, bits) {
        (true, 32) => PcmSampleFormat::F32,
        (false, 8) => PcmSampleFormat::U8,
        (false, 16) => PcmSampleFormat::I16,
        (false, 24) => PcmSampleFormat::I24,
        (false, 32) => PcmSampleFormat::I32,
        _ => {
            return Err(NexusError::Media(format!(
                "不支持的 ScreenCaptureKit 音频格式：{bits}-bit，float={is_float}"
            )))
        }
    };

    let buffers = sample
        .audio_buffer_list()
        .map_err(|status| NexusError::Media(format!("读取 SCK 音频缓冲失败：{status}")))?;
    convert_buffers(
        &buffers,
        sample_format,
        source_rate,
        source_channels,
        target_rate,
        target_channels,
    )
}

fn convert_buffers(
    buffers: &AudioBufferList,
    sample_format: PcmSampleFormat,
    source_rate: u32,
    source_channels: u16,
    target_rate: u32,
    target_channels: u16,
) -> Result<Vec<i16>> {
    if buffers.num_buffers() == 0 {
        return Ok(Vec::new());
    }

    if buffers.num_buffers() == 1 {
        let Some(buffer) = buffers.get(0) else {
            return Ok(Vec::new());
        };
        let channels = if buffer.number_channels > 0 {
            u16::try_from(buffer.number_channels).unwrap_or(source_channels)
        } else {
            source_channels
        };
        if channels == 0 || buffer.data().is_empty() {
            return Ok(Vec::new());
        }
        return Ok(convert_to_target(
            buffer.data(),
            sample_format,
            source_rate,
            channels,
            target_rate,
            target_channels,
        ));
    }

    let mut decoded = Vec::with_capacity(buffers.num_buffers());
    for buffer in buffers.iter() {
        if buffer.number_channels != 1 {
            return Err(NexusError::Media(
                "ScreenCaptureKit 多缓冲音频不是单声道平面格式".to_string(),
            ));
        }
        decoded.push(decode_to_i16(buffer.data(), sample_format));
    }
    let frames = decoded.iter().map(Vec::len).min().unwrap_or(0);
    let channels = u16::try_from(decoded.len()).unwrap_or(1);
    if frames == 0 || channels == 0 {
        return Ok(Vec::new());
    }
    let mut interleaved = Vec::with_capacity(frames * usize::from(channels));
    for frame in 0..frames {
        for buffer in &decoded {
            interleaved.push(buffer[frame]);
        }
    }
    let mapped = map_channels(&interleaved, channels, target_channels);
    Ok(resample_linear(
        &mapped,
        target_channels,
        source_rate,
        target_rate,
    ))
}

fn chunk_frame_count(sample_rate: u32, frame_ms: u32) -> usize {
    ((u64::from(sample_rate) * u64::from(frame_ms)) / 1_000).max(1) as usize
}

