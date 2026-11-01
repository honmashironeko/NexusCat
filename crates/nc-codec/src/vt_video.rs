use apple_cf::cm::CMTime;
use apple_cf::cv::CVPixelBuffer;
use apple_cf::iosurface::{IOSurface, IOSurfaceLockOptions};
use nc_core::{EncodedVideoFrame, NexusError, Result, VideoFrame};
use videotoolbox::compression::{
    CompressionSession, EncodedFrame, FrameProperties, HardwareAcceleration, ProfileLevel,
};
use videotoolbox::session::Codec;

use crate::color::bgra_to_bgra_scaled;
use crate::video::{EncoderConfig, EncoderPreset};

const START_CODE: [u8; 4] = [0, 0, 0, 1];
const DEFAULT_NAL_LENGTH: usize = 4;

pub struct VtH264Encoder {
    session: CompressionSession,
    surface: IOSurface,
    config: EncoderConfig,
    force_keyframe: bool,
    parameter_sets: Vec<Vec<u8>>,
    nal_length: usize,
}

impl VtH264Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self> {
        config.validate()?;
        let width = i32::try_from(config.width)
            .map_err(|_| NexusError::invalid("VideoToolbox 宽度超出 i32 范围"))?;
        let height = i32::try_from(config.height)
            .map_err(|_| NexusError::invalid("VideoToolbox 高度超出 i32 范围"))?;
        let bitrate = i32::try_from(config.bitrate_kbps.saturating_mul(1_000))
            .map_err(|_| NexusError::invalid("VideoToolbox 码率超出 i32 范围"))?;
        let keyframe_interval = i32::try_from(config.keyframe_interval)
            .map_err(|_| NexusError::invalid("VideoToolbox 关键帧间隔超出 i32 范围"))?;

        let build_session = |profile_level: ProfileLevel, set_quality: bool| {
            let mut builder = CompressionSession::builder(width, height, Codec::H264)
                .with_real_time(true)
                .with_allow_frame_reordering(false)
                .with_average_bit_rate(bitrate)
                .with_expected_frame_rate(f64::from(config.fps))
                .with_max_keyframe_interval(keyframe_interval)
                .with_profile_level(profile_level)
                .with_low_latency_rate_control(true)
                .with_hardware_acceleration(HardwareAcceleration::Required);
            if set_quality {
                builder = builder.with_quality(quality_for(config.preset));
            }
            builder.build()
        };
        let preferred_profile_level = profile_level_for(config);
        let session = match build_session(preferred_profile_level, true) {
            Ok(session) => session,
            Err(videotoolbox::error::VTError::SetPropertyFailed { key, .. })
                if key == "Quality" =>
            {
                tracing::warn!("VideoToolbox 硬件编码器不支持 Quality 属性，忽略该提示重试");
                build_session(preferred_profile_level, false)
                    .map_err(|error| vt_error("创建编码会话", error))?
            }
            Err(error)
                if preferred_profile_level != ProfileLevel::H264ConstrainedBaselineAutoLevel =>
            {
                tracing::warn!(
                    %error,
                    "VideoToolbox 不接受显式高帧率 profile/level，回退自动级别"
                );
                build_session(ProfileLevel::H264ConstrainedBaselineAutoLevel, true)
                    .map_err(|error| vt_error("创建编码会话", error))?
            }
            Err(error) => return Err(vt_error("创建编码会话", error)),
        };

        let surface = IOSurface::create(
            config.width as usize,
            config.height as usize,
            u32::from_be_bytes(*b"BGRA"),
            4,
        )
        .ok_or_else(|| NexusError::Media("创建 VideoToolbox IOSurface 失败".to_string()))?;

        Ok(Self {
            session,
            surface,
            config,
            force_keyframe: true,
            parameter_sets: Vec::new(),
            nal_length: DEFAULT_NAL_LENGTH,
        })
    }

    fn fill_surface(&mut self, frame: &VideoFrame) -> Result<()> {
        let pixels = bgra_to_bgra_scaled(frame, self.config.width, self.config.height)?;
        let row_bytes = self.config.width as usize * 4;
        let mut guard = self
            .surface
            .lock(IOSurfaceLockOptions::NONE)
            .map_err(|status| {
                NexusError::Media(format!("锁定 VideoToolbox IOSurface 失败：{status}"))
            })?;
        let surface_bytes_per_row = guard.bytes_per_row();
        let surface_height = guard.height();
        if surface_bytes_per_row < row_bytes || surface_height < self.config.height as usize {
            return Err(NexusError::Media(
                "VideoToolbox IOSurface 尺寸小于编码尺寸".to_string(),
            ));
        }
        let surface_data = unsafe { guard.as_slice_mut() }
            .ok_or_else(|| NexusError::Media("VideoToolbox IOSurface 不可写".to_string()))?;
        let required = surface_bytes_per_row
            .checked_mul(surface_height)
            .ok_or_else(|| NexusError::Media("VideoToolbox IOSurface 长度溢出".to_string()))?;
        if surface_data.len() < required {
            return Err(NexusError::Media(
                "VideoToolbox IOSurface 数据长度不足".to_string(),
            ));
        }
        for row in 0..self.config.height as usize {
            let source = &pixels[row * row_bytes..(row + 1) * row_bytes];
            let offset = row * surface_bytes_per_row;
            surface_data[offset..offset + row_bytes].copy_from_slice(source);
        }
        Ok(())
    }

    pub fn encode_pixel_buffer(
        &mut self,
        pixel_buffer: &CVPixelBuffer,
        timestamp_us: u64,
    ) -> Result<Option<EncodedVideoFrame>> {
        if pixel_buffer.width() != self.config.width as usize
            || pixel_buffer.height() != self.config.height as usize
        {
            return Err(NexusError::Media(format!(
                "VideoToolbox 输入尺寸 {}x{} 与编码尺寸 {}x{} 不一致",
                pixel_buffer.width(),
                pixel_buffer.height(),
                self.config.width,
                self.config.height
            )));
        }
        let Some(surface) = pixel_buffer.io_surface() else {
            return Err(NexusError::Media(
                "VideoToolbox 输入像素缓冲没有 IOSurface 后备存储".to_string(),
            ));
        };
        let timestamp = i64::try_from(timestamp_us).unwrap_or(i64::MAX);
        let properties = if self.force_keyframe {
            FrameProperties::new().with_force_key_frame(true)
        } else {
            FrameProperties::new()
        };
        let requested_keyframe = self.force_keyframe;
        let encoded = self
            .session
            .encode_with_properties(&surface, CMTime::new(timestamp, 1_000_000), properties)
            .map_err(|error| vt_error("编码像素缓冲", error))?;
        self.finish_encoded(encoded, timestamp_us, requested_keyframe)
    }

    fn finish_encoded(
        &mut self,
        encoded: EncodedFrame,
        timestamp_us: u64,
        requested_keyframe: bool,
    ) -> Result<Option<EncodedVideoFrame>> {
        if encoded.data.is_empty() {
            if requested_keyframe {
                self.force_keyframe = true;
            }
            return Ok(None);
        }
        self.force_keyframe = false;

        self.refresh_parameter_sets(&encoded);
        let keyframe = avcc_contains_keyframe(&encoded.data, self.nal_length);
        let data = convert_access_unit(
            &encoded.data,
            self.nal_length,
            &self.parameter_sets,
            keyframe,
        )?;
        Ok(Some(EncodedVideoFrame {
            data,
            timestamp_us,
            keyframe,
            width: self.config.width,
            height: self.config.height,
        }))
    }

    fn refresh_parameter_sets(&mut self, encoded: &EncodedFrame) {
        let Some(sample) = encoded.cm_sample_buffer() else {
            return;
        };
        let Some(format) = sample.format_description() else {
            return;
        };
        if let Ok(sets) = format.video_parameter_sets() {
            if !sets.parameter_sets.is_empty() {
                self.parameter_sets = sets.parameter_sets;
            }
            if let Ok(length) = usize::try_from(sets.nal_unit_header_length) {
                if (1..=4).contains(&length) {
                    self.nal_length = length;
                }
            }
        }
    }
}

impl crate::video::VideoEncoder for VtH264Encoder {
    fn encode(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoFrame>> {
        self.fill_surface(frame)?;
        let timestamp = i64::try_from(frame.timestamp_us).unwrap_or(i64::MAX);
        let properties = if self.force_keyframe {
            FrameProperties::new().with_force_key_frame(true)
        } else {
            FrameProperties::new()
        };
        let requested_keyframe = self.force_keyframe;
        let encoded = self
            .session
            .encode_with_properties(&self.surface, CMTime::new(timestamp, 1_000_000), properties)
            .map_err(|error| vt_error("编码帧", error))?;
        self.finish_encoded(encoded, frame.timestamp_us, requested_keyframe)
    }

    fn flush(&mut self) -> Result<Vec<EncodedVideoFrame>> {
        Ok(Vec::new())
    }

    fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    fn config(&self) -> EncoderConfig {
        self.config
    }

    fn backend_name(&self) -> &str {
        "VideoToolbox"
    }

    fn is_hardware(&self) -> bool {
        true
    }
}

fn convert_access_unit(
    data: &[u8],
    nal_length: usize,
    parameter_sets: &[Vec<u8>],
    keyframe: bool,
) -> Result<Vec<u8>> {
    if !(1..=4).contains(&nal_length) {
        return Err(NexusError::Media(format!(
            "VideoToolbox NAL 长度字段非法：{nal_length}"
        )));
    }
    let mut output = Vec::with_capacity(data.len() + parameter_set_bytes(parameter_sets));
    if keyframe {
        append_parameter_sets(&mut output, parameter_sets);
    }
    let mut offset = 0usize;
    while offset < data.len() {
        if offset + nal_length > data.len() {
            return Err(NexusError::Media(
                "VideoToolbox AVCC 数据缺少完整 NAL 长度".to_string(),
            ));
        }
        let mut current_length = 0usize;
        for byte in &data[offset..offset + nal_length] {
            current_length = (current_length << 8) | usize::from(*byte);
        }
        offset += nal_length;
        if current_length == 0 {
            continue;
        }
        if offset + current_length > data.len() {
            return Err(NexusError::Media(
                "VideoToolbox AVCC NAL 长度超出数据范围".to_string(),
            ));
        }
        output.extend_from_slice(&START_CODE);
        output.extend_from_slice(&data[offset..offset + current_length]);
        offset += current_length;
    }
    Ok(output)
}

fn profile_level_for(config: EncoderConfig) -> ProfileLevel {
    let level = crate::video::level_idc(config.width, config.height, config.fps);
    match level {
        51 => ProfileLevel::H264Baseline5_1,
        52 => ProfileLevel::H264Baseline5_2,
        _ => ProfileLevel::H264ConstrainedBaselineAutoLevel,
    }
}

fn quality_for(preset: EncoderPreset) -> f32 {
    match preset {
        EncoderPreset::Fast => 0.45,
        EncoderPreset::Balanced => 0.7,
        EncoderPreset::Quality => 0.9,
    }
}

fn parameter_set_bytes(parameter_sets: &[Vec<u8>]) -> usize {
    parameter_sets
        .iter()
        .map(|set| set.len().saturating_add(START_CODE.len()))
        .sum()
}

fn append_parameter_sets(output: &mut Vec<u8>, parameter_sets: &[Vec<u8>]) {
    for set in parameter_sets {
        if set.is_empty() {
            continue;
        }
        if !is_annex_b(set) {
            output.extend_from_slice(&START_CODE);
        }
        output.extend_from_slice(set);
    }
}

fn is_annex_b(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 1]) || data.starts_with(&START_CODE)
}

fn avcc_contains_keyframe(data: &[u8], nal_length: usize) -> bool {
    if nal_length == 0 {
        return false;
    }
    let mut offset = 0usize;
    while offset + nal_length <= data.len() {
        let mut length = 0usize;
        for byte in &data[offset..offset + nal_length] {
            length = (length << 8) | usize::from(*byte);
        }
        offset += nal_length;
        if length == 0 || offset + length > data.len() {
            return false;
        }
        let nal_type = data[offset] & 0x1F;
        if nal_type == 5 || nal_type == 7 {
            return true;
        }
        offset += length;
    }
    false
}

fn vt_error(action: &str, error: impl std::fmt::Display) -> NexusError {
    NexusError::Media(format!("VideoToolbox {action}失败：{error}"))
}
