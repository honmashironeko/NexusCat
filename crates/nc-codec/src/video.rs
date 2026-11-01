use nc_core::{EncodedVideoFrame, NexusError, Result, VideoFrame};
use rusty_h264::{
    Encoder as H264Encoder, EncoderConfig as H264EncoderConfig, Preset as H264Preset,
};

use crate::bgra_to_i420;

const MAX_WIDTH: u32 = 7680;
const MAX_HEIGHT: u32 = 4320;
const MAX_FPS: u32 = 240;
const MAX_BITRATE_KBPS: u32 = 200_000;
const MAX_KEYFRAME_INTERVAL: u32 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncoderPreset {
    #[default]
    Fast,
    Balanced,
    Quality,
}

impl From<EncoderPreset> for H264Preset {
    fn from(preset: EncoderPreset) -> Self {
        match preset {
            EncoderPreset::Fast => Self::Fast,
            EncoderPreset::Balanced => Self::Balanced,
            EncoderPreset::Quality => Self::Quality,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub keyframe_interval: u32,
    pub preset: EncoderPreset,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 20_000,
            keyframe_interval: 120,
            preset: EncoderPreset::Fast,
        }
    }
}

impl EncoderConfig {
    pub fn validate(&self) -> Result<()> {
        validate_dimensions(self.width, self.height)?;
        if !(1..=MAX_FPS).contains(&self.fps) {
            return Err(NexusError::invalid(format!(
                "帧率必须在 1..={MAX_FPS} 之间"
            )));
        }
        if !(1..=MAX_BITRATE_KBPS).contains(&self.bitrate_kbps) {
            return Err(NexusError::invalid(format!(
                "码率必须在 1..={MAX_BITRATE_KBPS} kbps 之间"
            )));
        }
        if !(1..=MAX_KEYFRAME_INTERVAL).contains(&self.keyframe_interval) {
            return Err(NexusError::invalid(format!(
                "关键帧间隔必须在 1..={MAX_KEYFRAME_INTERVAL} 帧之间"
            )));
        }
        Ok(())
    }
}

pub trait VideoEncoder: Send {
    fn encode(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoFrame>>;

    fn poll(&mut self) -> Result<Vec<EncodedVideoFrame>> {
        Ok(Vec::new())
    }

    fn flush(&mut self) -> Result<Vec<EncodedVideoFrame>>;

    fn request_keyframe(&mut self);

    fn config(&self) -> EncoderConfig;

    fn backend_name(&self) -> &str {
        "unknown"
    }

    fn is_hardware(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderVendor {
    Nvidia,
    Amd,
    Intel,
}

impl EncoderVendor {
    pub fn from_name(name: &str) -> Option<Self> {
        let normalized = name.to_ascii_lowercase();
        if normalized.contains("nvidia") || normalized.contains("nvenc") {
            Some(Self::Nvidia)
        } else if normalized.contains("amd")
            || normalized.contains("amf")
            || normalized.contains("radeon")
        {
            Some(Self::Amd)
        } else if normalized.contains("intel")
            || normalized.contains("quick sync")
            || normalized.contains("quicksync")
        {
            Some(Self::Intel)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderProbe {
    pub hardware: bool,
    pub backend: String,
}

pub fn probe_video_encoder_with_preference(
    preferred_vendor: Option<EncoderVendor>,
) -> EncoderProbe {
    let config = EncoderConfig {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_kbps: 4_000,
        keyframe_interval: 60,
        preset: EncoderPreset::Fast,
    };
    match create_video_encoder_with_preference(config, preferred_vendor) {
        Ok(encoder) => EncoderProbe {
            hardware: encoder.is_hardware(),
            backend: encoder.backend_name().to_string(),
        },
        Err(error) => {
            tracing::warn!(%error, "H.264 编码器能力探测失败");
            EncoderProbe {
                hardware: false,
                backend: "unavailable".to_string(),
            }
        }
    }
}

pub trait VideoDecoder: Send {
    fn decode(&mut self, access_unit: &[u8], timestamp_us: u64) -> Result<Option<VideoFrame>>;

    fn flush(&mut self) -> Result<Vec<VideoFrame>>;
}

pub fn create_video_encoder(config: EncoderConfig) -> Result<Box<dyn VideoEncoder>> {
    create_video_encoder_with_preference(config, None)
}

pub fn create_video_encoder_with_preference(
    config: EncoderConfig,
    preferred_vendor: Option<EncoderVendor>,
) -> Result<Box<dyn VideoEncoder>> {
    create_video_encoder_tuned(config, preferred_vendor, false)
}

#[cfg_attr(not(windows), allow(unused_variables))]
pub fn create_video_encoder_tuned(
    config: EncoderConfig,
    preferred_vendor: Option<EncoderVendor>,
    prefer_speed: bool,
) -> Result<Box<dyn VideoEncoder>> {
    config.validate()?;
    #[cfg(windows)]
    let mut tried_openh264 = false;
    #[cfg(windows)]
    if prefer_speed && !force_software_backend() {
        tried_openh264 = true;
        match crate::openh264_video::OpenH264Encoder::new(config) {
            Ok(encoder) => {
                tracing::info!(
                    backend = encoder.backend_name(),
                    hardware = false,
                    "使用 OpenH264 软件编码器"
                );
                return Ok(Box::new(encoder));
            }
            Err(error) => {
                tracing::warn!(%error, "OpenH264 编码器不可用，回退 Media Foundation/软件编码");
            }
        }
    }
    #[cfg(windows)]
    if !force_software_backend() {
        match crate::mf_video::MfH264Encoder::new_with_preference_and_speed(
            config,
            preferred_vendor,
            prefer_speed,
        ) {
            Ok(encoder) => {
                tracing::info!(
                    backend = encoder.backend_name(),
                    hardware = encoder.is_hardware(),
                    "使用 Media Foundation H.264 编码器"
                );
                return Ok(Box::new(encoder));
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Media Foundation H.264 编码器不可用，尝试 OpenH264 软件回退"
                );
            }
        }
    }
    #[cfg(windows)]
    if !tried_openh264 && !force_software_backend() {
        match crate::openh264_video::OpenH264Encoder::new(config) {
            Ok(encoder) => {
                tracing::info!(
                    backend = encoder.backend_name(),
                    hardware = false,
                    "硬件编码不可用，使用 OpenH264 软件回退"
                );
                return Ok(Box::new(encoder));
            }
            Err(error) => {
                tracing::warn!(%error, "OpenH264 回退不可用，最后使用 rusty_h264");
            }
        }
    }
    #[cfg(target_os = "macos")]
    if !force_software_backend() {
        match crate::vt_video::VtH264Encoder::new(config) {
            Ok(encoder) => {
                tracing::info!(
                    backend = encoder.backend_name(),
                    hardware = encoder.is_hardware(),
                    "使用 VideoToolbox H.264 编码器"
                );
                return Ok(Box::new(encoder));
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "VideoToolbox H.264 编码器不可用，回退到 rusty_h264 软件编码"
                );
            }
        }
    }
    create_software_video_encoder(config)
}

pub fn create_software_video_encoder(config: EncoderConfig) -> Result<Box<dyn VideoEncoder>> {
    config.validate()?;
    let raw_config = to_h264_config(config);
    let encoder = H264Encoder::new(raw_config)
        .map_err(|error| NexusError::Media(format!("初始化 H.264 编码器失败：{error}")))?;
    Ok(Box::new(RustyH264Encoder {
        encoder,
        config,
        last_timestamp_us: 0,
    }))
}

#[cfg(any(windows, target_os = "macos"))]
fn force_software_backend() -> bool {
    std::env::var_os("NEXUSCAT_FORCE_SOFTWARE").is_some_and(|value| value.to_string_lossy() == "1")
}

pub fn create_video_decoder(width: u32, height: u32) -> Result<Box<dyn VideoDecoder>> {
    validate_dimensions(width, height)?;
    Ok(Box::new(RustyH264Decoder {
        decoder: rusty_h264::Decoder::new(),
        width,
        height,
    }))
}

struct RustyH264Encoder {
    encoder: H264Encoder,
    config: EncoderConfig,
    last_timestamp_us: u64,
}

impl VideoEncoder for RustyH264Encoder {
    fn encode(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoFrame>> {
        let yuv = bgra_to_i420(frame, self.config.width, self.config.height)?;
        let data = self
            .encoder
            .try_encode(&yuv)
            .map_err(|error| NexusError::Media(format!("H.264 编码失败：{error}")))?;
        self.last_timestamp_us = frame.timestamp_us;
        if data.is_empty() {
            return Ok(None);
        }
        let keyframe = contains_keyframe_nal(&data);
        Ok(Some(EncodedVideoFrame {
            data,
            timestamp_us: frame.timestamp_us,
            keyframe,
            width: self.config.width,
            height: self.config.height,
        }))
    }

    fn flush(&mut self) -> Result<Vec<EncodedVideoFrame>> {
        let data = self
            .encoder
            .try_flush()
            .map_err(|error| NexusError::Media(format!("H.264 冲刷失败：{error}")))?;
        if data.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![EncodedVideoFrame {
            keyframe: contains_keyframe_nal(&data),
            data,
            timestamp_us: self.last_timestamp_us,
            width: self.config.width,
            height: self.config.height,
        }])
    }

    fn request_keyframe(&mut self) {
        self.encoder.request_keyframe();
    }

    fn config(&self) -> EncoderConfig {
        self.config
    }

    fn backend_name(&self) -> &str {
        "rusty_h264"
    }
}

struct RustyH264Decoder {
    decoder: rusty_h264::Decoder,
    width: u32,
    height: u32,
}

impl VideoDecoder for RustyH264Decoder {
    fn decode(&mut self, access_unit: &[u8], timestamp_us: u64) -> Result<Option<VideoFrame>> {
        let decoded = self
            .decoder
            .decode(access_unit)
            .map_err(|error| NexusError::Media(format!("H.264 解码失败：{error}")))?;
        let Some(yuv) = decoded else {
            return Ok(None);
        };
        if yuv.width != self.width as usize || yuv.height != self.height as usize {
            return Err(NexusError::Media(format!(
                "解码尺寸 {}x{} 与协商尺寸 {}x{} 不一致",
                yuv.width, yuv.height, self.width, self.height
            )));
        }
        let mut frame = crate::i420_to_bgra(&yuv)?;
        frame.timestamp_us = timestamp_us;
        Ok(Some(frame))
    }

    fn flush(&mut self) -> Result<Vec<VideoFrame>> {
        Ok(Vec::new())
    }
}

fn to_h264_config(config: EncoderConfig) -> H264EncoderConfig {
    let mut raw = H264EncoderConfig::baseline(config.width as usize, config.height as usize);
    raw.level_idc = level_idc(config.width, config.height, config.fps);
    raw.gop_size = config.keyframe_interval;
    raw.min_keyint = 1;
    raw.scenecut = 0;
    raw.lookahead = 0;
    raw.bitrate = config.bitrate_kbps.saturating_mul(1000);
    raw.framerate = config.fps as f32;
    raw.num_ref_frames = 1;
    raw.preset = config.preset.into();
    raw.mbtree = false;
    raw
}

fn validate_dimensions(width: u32, height: u32) -> Result<()> {
    if width < 2 || height < 2 {
        return Err(NexusError::invalid("视频宽高必须至少为 2"));
    }
    if width > MAX_WIDTH || height > MAX_HEIGHT {
        return Err(NexusError::invalid(format!(
            "视频尺寸不能超过 {MAX_WIDTH}x{MAX_HEIGHT}"
        )));
    }
    if width % 2 != 0 || height % 2 != 0 {
        return Err(NexusError::invalid("H.264 要求宽高为偶数"));
    }
    Ok(())
}

pub(crate) fn level_idc(width: u32, height: u32, fps: u32) -> u8 {
    let macroblocks = width.div_ceil(16) * height.div_ceil(16);
    let macroblocks_per_second = macroblocks.saturating_mul(fps);
    match macroblocks_per_second {
        0..=108_000 => 31,
        108_001..=245_760 => 40,
        245_761..=522_240 => 42,
        522_241..=589_824 => 50,
        589_825..=983_040 => 51,
        _ => 52,
    }
}

pub(crate) fn any_nal_type(data: &[u8], mut predicate: impl FnMut(u8) -> bool) -> bool {
    let mut index = 0;
    while index + 3 < data.len() {
        let header = if data[index] == 0 && data[index + 1] == 0 {
            if data[index + 2] == 1 {
                Some(index + 3)
            } else if index + 4 < data.len() && data[index + 2] == 0 && data[index + 3] == 1 {
                Some(index + 4)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(header) = header {
            if predicate(data[header] & 0x1f) {
                return true;
            }
            index = header + 1;
        } else {
            index += 1;
        }
    }
    false
}

pub(crate) fn contains_keyframe_nal(data: &[u8]) -> bool {
    any_nal_type(data, |nal_type| nal_type == 5 || nal_type == 7)
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn contains_nal_type(data: &[u8], target: u8) -> bool {
    any_nal_type(data, |nal_type| nal_type == target)
}
