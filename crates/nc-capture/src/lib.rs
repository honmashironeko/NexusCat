#[cfg(any(windows, target_os = "macos"))]
mod audio_convert;
#[cfg(any(windows, target_os = "macos"))]
mod frame_loop;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;
#[cfg(windows)]
mod windows;
#[cfg(any(windows, target_os = "macos"))]
mod worker;

#[cfg(target_os = "macos")]
use self::macos as platform;
#[cfg(not(any(windows, target_os = "macos")))]
use self::unsupported as platform;
#[cfg(windows)]
use self::windows as platform;
#[cfg(windows)]
pub use self::windows::gpu::{disable_gpu_capture, GpuFrame};
#[cfg(target_os = "macos")]
pub use self::macos::disable_pixel_buffer_capture;

use crossbeam_channel::Sender;
use nc_core::{AudioFrame, PixelFormat, Result, VideoFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenCaptureConfig {
    pub monitor_id: u32,
    pub fps: u32,
    pub include_cursor: bool,
    pub target_width: u32,
    pub target_height: u32,
    pub excluded_window_id: Option<u32>,
    pub prefer_gpu_pixels: bool,
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct PixelBufferFrame {
    pub pixel_buffer: screencapturekit::cv::CVPixelBuffer,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub struct CapturedFrame {
    pub frame: VideoFrame,
    #[cfg(windows)]
    pub gpu: Option<GpuFrame>,
    #[cfg(target_os = "macos")]
    pub pixel_buffer: Option<PixelBufferFrame>,
    pub captured_at_us: u64,
    pub monitor_id: u32,
}

pub trait ScreenCapturer: Send {
    fn start(&mut self, config: ScreenCaptureConfig, sink: Sender<CapturedFrame>) -> Result<()>;

    fn stop(&mut self);

    fn is_running(&self) -> bool;
}

pub fn create_screen_capturer() -> Result<Box<dyn ScreenCapturer>> {
    platform::create_screen_capturer()
}

pub fn black_frame(width: u32, height: u32, timestamp_us: u64) -> VideoFrame {
    let stride = width as usize * 4;
    let data_len = stride.saturating_mul(height as usize);
    VideoFrame {
        width,
        height,
        stride,
        format: PixelFormat::Bgra8,
        timestamp_us,
        data: vec![0; data_len],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioCaptureConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_ms: u32,
}

impl Default for AudioCaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            frame_ms: 20,
        }
    }
}

pub trait AudioCapturer: Send {
    fn start(&mut self, config: AudioCaptureConfig, sink: Sender<AudioFrame>) -> Result<()>;

    fn stop(&mut self);

    fn is_running(&self) -> bool;
}

pub fn create_audio_capturer() -> Result<Box<dyn AudioCapturer>> {
    platform::create_audio_capturer()
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) const STARTUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn unix_time_us() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

