use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Bgra8,
    Rgba8,
    Nv12,
    I420,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            Self::Bgra8 | Self::Rgba8 => Some(4),
            Self::Nv12 | Self::I420 => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
    pub timestamp_us: u64,
    pub data: Vec<u8>,
}

impl VideoFrame {
    pub fn is_consistent(&self) -> bool {
        match self.format.bytes_per_pixel() {
            Some(bytes_per_pixel) => {
                self.height as usize * self.stride == self.data.len()
                    && self.stride >= self.width as usize * bytes_per_pixel
            }
            None => !self.data.is_empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodedVideoFrame {
    pub data: Vec<u8>,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    PcmS16Le,
    Opus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioFrame {
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp_us: u64,
    pub samples: Vec<i16>,
}

impl AudioFrame {
    pub fn frames_per_channel(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    pub fn duration_us(&self) -> u64 {
        if self.sample_rate == 0 {
            0
        } else {
            self.frames_per_channel() as u64 * 1_000_000 / self.sample_rate as u64
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodedAudioFrame {
    pub data: Vec<u8>,
    pub timestamp_us: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub codec: AudioCodec,
}

