use std::ffi::c_void;

use crossbeam_channel::Sender;
use nc_core::{NexusError, PixelFormat, Result, VideoFrame};
use tracing::debug;

use crate::frame_loop::run_frame_loop;
use crate::worker::CaptureWorker;
use crate::{CapturedFrame, ScreenCaptureConfig, ScreenCapturer, STARTUP_TIMEOUT};

const MAX_FPS: u32 = 240;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDisplayCreateImage(display: u32) -> *mut c_void;
    fn CGImageGetWidth(image: *mut c_void) -> usize;
    fn CGImageGetHeight(image: *mut c_void) -> usize;
    fn CGImageGetBytesPerRow(image: *mut c_void) -> usize;
    fn CGImageGetBitsPerPixel(image: *mut c_void) -> usize;
    fn CGImageGetDataProvider(image: *mut c_void) -> *mut c_void;
    fn CGDataProviderCopyData(provider: *mut c_void) -> *mut c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataGetBytePtr(data: *mut c_void) -> *const u8;
    fn CFDataGetLength(data: *mut c_void) -> isize;
    fn CFRelease(value: *const c_void);
}

pub(crate) struct CgScreenCapturer {
    worker: CaptureWorker,
}

impl CgScreenCapturer {
    pub(crate) fn new() -> Self {
        Self {
            worker: CaptureWorker::new(),
        }
    }
}

impl ScreenCapturer for CgScreenCapturer {
    fn start(&mut self, config: ScreenCaptureConfig, sink: Sender<CapturedFrame>) -> Result<()> {
        validate_config(&config)?;
        let monitors = nc_platform::monitors()?;
        if !monitors
            .iter()
            .any(|monitor| monitor.id == config.monitor_id)
        {
            return Err(NexusError::NotFound(format!(
                "未找到显示器 {}",
                config.monitor_id
            )));
        }
        if config.include_cursor {
            debug!("CoreGraphics 屏幕采集不包含光标，include_cursor 已忽略");
        }

        self.worker
            .spawn("nc-capture-cg", STARTUP_TIMEOUT, move |running, startup| {
                let mut source = CgScreenSource::new(config.monitor_id);
                let _ = startup.send(Ok(()));
                run_frame_loop(config.fps, &sink, &running, || source.capture());
            })
    }

    fn stop(&mut self) {
        self.worker.stop();
    }

    fn is_running(&self) -> bool {
        self.worker.is_running()
    }
}

fn validate_config(config: &ScreenCaptureConfig) -> Result<()> {
    if config.fps == 0 || config.fps > MAX_FPS {
        return Err(NexusError::invalid(format!(
            "屏幕采集 fps 必须在 1..={MAX_FPS}，实际为 {}",
            config.fps
        )));
    }
    Ok(())
}

struct CgScreenSource {
    monitor_id: u32,
}

impl CgScreenSource {
    fn new(monitor_id: u32) -> Self {
        Self { monitor_id }
    }

    fn capture(&mut self) -> Result<CapturedFrame> {
        let image = unsafe { CGDisplayCreateImage(self.monitor_id) };
        if image.is_null() {
            return Err(NexusError::platform(
                "CGDisplayCreateImage 失败：无法获取显示器图像（请检查屏幕录制权限）",
            ));
        }
        let image = CoreFoundationRef(image);
        let frame = copy_image(image.0)?;
        let timestamp_us = crate::unix_time_us();

        Ok(CapturedFrame {
            frame: VideoFrame {
                timestamp_us,
                ..frame
            },
            pixel_buffer: None,
            captured_at_us: timestamp_us,
            monitor_id: self.monitor_id,
        })
    }
}

fn copy_image(image: *mut c_void) -> Result<VideoFrame> {
    let width = unsafe { CGImageGetWidth(image) };
    let height = unsafe { CGImageGetHeight(image) };
    let bytes_per_row = unsafe { CGImageGetBytesPerRow(image) };
    let bits_per_pixel = unsafe { CGImageGetBitsPerPixel(image) };
    if width == 0 || height == 0 {
        return Err(NexusError::platform("CGDisplayCreateImage 返回空尺寸图像"));
    }
    if bits_per_pixel != 32 {
        return Err(NexusError::Media(format!(
            "不支持的 CGImage 像素深度：{bits_per_pixel}-bit（仅支持 32-bit BGRA）"
        )));
    }
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| NexusError::platform("CGImage 行宽溢出"))?;
    if bytes_per_row < row_bytes {
        return Err(NexusError::platform(format!(
            "CGImage 行宽异常：{bytes_per_row} < {row_bytes}"
        )));
    }
    let required_bytes = bytes_per_row
        .checked_mul(height)
        .ok_or_else(|| NexusError::platform("CGImage 数据长度溢出"))?;
    let frame_bytes = row_bytes
        .checked_mul(height)
        .ok_or_else(|| NexusError::platform("CGImage 帧长度溢出"))?;

    let provider = unsafe { CGImageGetDataProvider(image) };
    if provider.is_null() {
        return Err(NexusError::platform("CGImage 缺少数据提供者"));
    }
    let data = CoreFoundationRef(unsafe { CGDataProviderCopyData(provider) });
    if data.0.is_null() {
        return Err(NexusError::platform("CGDataProviderCopyData 返回空数据"));
    }

    let length = unsafe { CFDataGetLength(data.0) };
    let pointer = unsafe { CFDataGetBytePtr(data.0) };
    if pointer.is_null() || length < 0 || (length as usize) < required_bytes {
        return Err(NexusError::platform(format!(
            "CGImage 数据长度不足：{length} < {required_bytes}"
        )));
    }

    let mut pixels = vec![0u8; frame_bytes];
    for row in 0..height {
        let source =
            unsafe { std::slice::from_raw_parts(pointer.add(row * bytes_per_row), row_bytes) };
        let target = &mut pixels[row * row_bytes..(row + 1) * row_bytes];
        target.copy_from_slice(source);
        for pixel in target.chunks_exact_mut(4) {
            pixel[3] = 0xFF;
        }
    }

    Ok(VideoFrame {
        width: u32::try_from(width)
            .map_err(|_| NexusError::platform("CGImage 宽度超出 VideoFrame 范围"))?,
        height: u32::try_from(height)
            .map_err(|_| NexusError::platform("CGImage 高度超出 VideoFrame 范围"))?,
        stride: row_bytes,
        format: PixelFormat::Bgra8,
        timestamp_us: 0,
        data: pixels,
    })
}

struct CoreFoundationRef(*mut c_void);

impl Drop for CoreFoundationRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CFRelease(self.0.cast_const());
            }
        }
    }
}
