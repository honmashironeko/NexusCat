use nc_core::{EncodedVideoFrame, NexusError, Result, VideoFrame};
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig as OpenH264Config, FrameRate, IntraFramePeriod,
    Profile, RateControlMode, SpsPpsStrategy, UsageType, VuiConfig,
};
use openh264::formats::YUVSlices;
use openh264::{OpenH264API, Timestamp};

use crate::color::bgra_to_i420;
use crate::video::{contains_keyframe_nal, EncoderConfig, VideoEncoder};

pub struct OpenH264Encoder {
    encoder: Encoder,
    config: EncoderConfig,
    force_keyframe: bool,
    timestamp_ms: u64,
}

impl OpenH264Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self> {
        config.validate()?;
        let openh264_config = OpenH264Config::new()
            .bitrate(BitRate::from_bps(config.bitrate_kbps.saturating_mul(1_000)))
            .max_frame_rate(FrameRate::from_hz(config.fps as f32))
            .usage_type(UsageType::ScreenContentRealTime)
            .complexity(Complexity::Low)
            .profile(Profile::Baseline)
            .rate_control_mode(RateControlMode::Bitrate)
            .num_threads(0)
            .max_slice_len(32_768)
            .skip_frames(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(config.keyframe_interval))
            .sps_pps_strategy(SpsPpsStrategy::SpsPpsListing)
            .vui(VuiConfig::bt709());
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), openh264_config)
            .map_err(|error| NexusError::Media(format!("创建 OpenH264 编码器失败：{error}")))?;
        Ok(Self {
            encoder,
            config,
            force_keyframe: true,
            timestamp_ms: 0,
        })
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn encode(&mut self, frame: &VideoFrame) -> Result<Option<EncodedVideoFrame>> {
        let yuv = bgra_to_i420(frame, self.config.width, self.config.height)?;
        let width = self.config.width as usize;
        let height = self.config.height as usize;
        let source = YUVSlices::new(
            (yuv.y.as_slice(), yuv.u.as_slice(), yuv.v.as_slice()),
            (width, height),
            (width, width / 2, width / 2),
        );
        if self.force_keyframe {
            self.encoder.force_intra_frame();
            self.force_keyframe = false;
        }
        let timestamp = Timestamp::from_millis(self.timestamp_ms);
        self.timestamp_ms = self
            .timestamp_ms
            .saturating_add(1_000 / u64::from(self.config.fps.max(1)));
        let bitstream = self
            .encoder
            .encode_at(&source, timestamp)
            .map_err(|error| NexusError::Media(format!("OpenH264 编码失败：{error}")))?;
        let data = bitstream.to_vec();
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
        Ok(Vec::new())
    }

    fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    fn config(&self) -> EncoderConfig {
        self.config
    }

    fn backend_name(&self) -> &str {
        "openh264"
    }
}
