use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Sender, TrySendError};
use nc_core::{NexusError, PixelFormat, Result, VideoFrame};
use screencapturekit::cm::{
    CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, CMTime, SCFrameStatus,
};
use screencapturekit::prelude::{
    PixelFormat as SckPixelFormat, SCContentFilter, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamOutputTrait, SCStreamOutputType, SCWindow,
};
use tracing::{debug, warn};

use super::pixel_buffer_capture_disabled;
use crate::worker::CaptureWorker;
use crate::{CapturedFrame, ScreenCaptureConfig, ScreenCapturer, STARTUP_TIMEOUT};

const MAX_FPS: u32 = 240;
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const NV12_420V: u32 = u32::from_be_bytes(*b"420v");

pub(crate) struct SckScreenCapturer {
    worker: CaptureWorker,
}

impl SckScreenCapturer {
    pub(crate) fn new() -> Self {
        Self {
            worker: CaptureWorker::new(),
        }
    }
}

impl ScreenCapturer for SckScreenCapturer {
    fn start(&mut self, config: ScreenCaptureConfig, sink: Sender<CapturedFrame>) -> Result<()> {
        validate_config(&config)?;
        self.worker.spawn(
            "nc-capture-sck",
            STARTUP_TIMEOUT,
            move |running, startup| match start_stream(config, sink) {
                Ok(stream) => {
                    let _ = startup.send(Ok(()));
                    while running.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(10));
                    }
                    if let Err(error) = stream.stop_capture() {
                        debug!(%error, "停止 ScreenCaptureKit 流失败");
                    }
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

fn validate_config(config: &ScreenCaptureConfig) -> Result<()> {
    if config.fps == 0 || config.fps > MAX_FPS {
        return Err(NexusError::invalid(format!(
            "屏幕采集 fps 必须在 1..={MAX_FPS}，实际为 {}",
            config.fps
        )));
    }
    Ok(())
}

fn start_stream(config: ScreenCaptureConfig, sink: Sender<CapturedFrame>) -> Result<SCStream> {
    let content = SCShareableContent::get().map_err(|error| {
        NexusError::platform(format!("获取 ScreenCaptureKit 内容失败：{error}"))
    })?;
    let display = content
        .displays()
        .into_iter()
        .find(|display| display.display_id() == config.monitor_id)
        .ok_or_else(|| {
            NexusError::NotFound(format!(
                "ScreenCaptureKit 未找到显示器 {}",
                config.monitor_id
            ))
        })?;
    let excluded_windows: Vec<SCWindow> = config
        .excluded_window_id
        .map(|id| {
            content
                .windows()
                .into_iter()
                .filter(|window| window.window_id() == id)
                .collect()
        })
        .unwrap_or_default();
    let excluded_refs: Vec<&SCWindow> = excluded_windows.iter().collect();
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&excluded_refs)
        .build()
        .map_err(|error| NexusError::platform(format!("创建 SCK 内容过滤器失败：{error}")))?;

    let width = if config.target_width == 0 {
        display.width()
    } else {
        config.target_width
    };
    let height = if config.target_height == 0 {
        display.height()
    } else {
        config.target_height
    };
    if width == 0 || height == 0 {
        return Err(NexusError::platform("ScreenCaptureKit 采集尺寸为 0"));
    }

    let pixel_format = if config.prefer_gpu_pixels {
        SckPixelFormat::YCbCr_420v
    } else {
        SckPixelFormat::BGRA
    };
    let mut stream_config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        .with_pixel_format(pixel_format)
        .with_shows_cursor(config.include_cursor)
        .with_scales_to_fit(true)
        .with_queue_depth(3)
        .with_minimum_frame_interval(&CMTime::new(1, config.fps as i32));
    if let Err(error) = stream_config.set_preserves_aspect_ratio(false) {
        warn!(
            %error,
            "ScreenCaptureKit 无法关闭宽高比保持；输出比例与显示器不一致时可能留边"
        );
    }

    let mut stream = SCStream::new(&filter, &stream_config)
        .map_err(|error| NexusError::platform(format!("创建 ScreenCaptureKit 流失败：{error}")))?;
    stream
        .add_output_handler(
            ScreenHandler {
                sink,
                monitor_id: config.monitor_id,
                use_pixel_buffer: config.prefer_gpu_pixels,
            },
            SCStreamOutputType::Screen,
        )
        .map_err(|error| NexusError::platform(format!("注册 SCK 屏幕输出失败：{error}")))?;
    stream
        .start_capture()
        .map_err(|error| NexusError::platform(format!("启动 ScreenCaptureKit 流失败：{error}")))?;
    Ok(stream)
}

struct ScreenHandler {
    sink: Sender<CapturedFrame>,
    monitor_id: u32,
    use_pixel_buffer: bool,
}

impl SCStreamOutputTrait for ScreenHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        if output_type != SCStreamOutputType::Screen {
            return;
        }
        if !sample
            .frame_status()
            .is_some_and(SCFrameStatus::has_content)
        {
            return;
        }
        let Some(pixel_buffer) = sample.pixel_buffer() else {
            return;
        };
        if self.use_pixel_buffer {
            if pixel_buffer.pixel_format() != NV12_420V {
                warn!(
                    format = format_args!("{:#x}", pixel_buffer.pixel_format()),
                    "ScreenCaptureKit 返回了非 420v 像素格式，丢弃该帧"
                );
                return;
            }
            let (Ok(width), Ok(height)) = (
                u32::try_from(pixel_buffer.width()),
                u32::try_from(pixel_buffer.height()),
            ) else {
                return;
            };
            if width < 2 || height < 2 {
                return;
            }
            let timestamp_us = crate::unix_time_us();
            if pixel_buffer_capture_disabled() {
                let Some(frame) =
                    nv12_pixel_buffer_to_bgra(&pixel_buffer, timestamp_us, self.monitor_id)
                else {
                    return;
                };
                self.send_frame(frame);
                return;
            }
            let frame = CapturedFrame {
                frame: VideoFrame {
                    width,
                    height,
                    stride: width as usize * 4,
                    format: PixelFormat::Bgra8,
                    timestamp_us,
                    data: Vec::new(),
                },
                pixel_buffer: Some(crate::PixelBufferFrame {
                    pixel_buffer,
                    width,
                    height,
                }),
                captured_at_us: timestamp_us,
                monitor_id: self.monitor_id,
            };
            self.send_frame(frame);
            return;
        }
        if pixel_buffer.pixel_format() != BGRA {
            warn!(
                format = format_args!("{:#x}", pixel_buffer.pixel_format()),
                "ScreenCaptureKit 返回了非 BGRA 像素格式，丢弃该帧"
            );
            return;
        }
        let width = pixel_buffer.width();
        let height = pixel_buffer.height();
        let Ok(guard) = pixel_buffer.lock_read_only() else {
            debug!("锁定 ScreenCaptureKit 像素缓冲区失败");
            return;
        };
        let bytes_per_row = guard.bytes_per_row();
        let Some(source) = (unsafe { guard.as_slice() }) else {
            debug!("读取 ScreenCaptureKit 像素缓冲区失败");
            return;
        };
        let Some(row_bytes) = width.checked_mul(4) else {
            return;
        };
        if width == 0 || height == 0 || bytes_per_row < row_bytes {
            return;
        }
        let Some(frame_bytes) = row_bytes.checked_mul(height) else {
            return;
        };
        let mut pixels = vec![0u8; frame_bytes];
        for row in 0..height {
            let source_start = row * bytes_per_row;
            let source_end = source_start + row_bytes;
            if source_end > source.len() {
                return;
            }
            let target_start = row * row_bytes;
            pixels[target_start..target_start + row_bytes]
                .copy_from_slice(&source[source_start..source_end]);
            for pixel in pixels[target_start..target_start + row_bytes].chunks_exact_mut(4) {
                pixel[3] = 0xFF;
            }
        }
        let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
            return;
        };
        let timestamp_us = crate::unix_time_us();
        let frame = CapturedFrame {
            frame: VideoFrame {
                width,
                height,
                stride: row_bytes,
                format: PixelFormat::Bgra8,
                timestamp_us,
                data: pixels,
            },
            pixel_buffer: None,
            captured_at_us: timestamp_us,
            monitor_id: self.monitor_id,
        };
        self.send_frame(frame);
    }
}

impl ScreenHandler {
    fn send_frame(&self, frame: CapturedFrame) {
        match self.sink.try_send(frame) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => debug!("屏幕帧接收端拥塞，丢弃一帧"),
            Err(TrySendError::Disconnected(_)) => debug!("屏幕帧接收端已关闭"),
        }
    }
}

fn nv12_pixel_buffer_to_bgra(
    pixel_buffer: &screencapturekit::cv::CVPixelBuffer,
    timestamp_us: u64,
    monitor_id: u32,
) -> Option<CapturedFrame> {
    let width = pixel_buffer.width();
    let height = pixel_buffer.height();
    if width < 4 || height < 2 || width % 2 != 0 || height % 2 != 0 {
        return None;
    }
    let guard = pixel_buffer.lock_read_only().ok()?;
    let y_plane = unsafe { guard.plane_data(0) }?;
    let uv_plane = unsafe { guard.plane_data(1) }?;
    let y_stride = guard.bytes_per_row_of_plane(0);
    let uv_stride = guard.bytes_per_row_of_plane(1);
    let pixels = nv12_to_bgra(y_plane, y_stride, uv_plane, uv_stride, width, height);
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        return None;
    };
    Some(CapturedFrame {
        frame: VideoFrame {
            width,
            height,
            stride: width as usize * 4,
            format: PixelFormat::Bgra8,
            timestamp_us,
            data: pixels,
        },
        pixel_buffer: None,
        captured_at_us: timestamp_us,
        monitor_id,
    })
}

fn nv12_to_bgra(
    y_plane: &[u8],
    y_stride: usize,
    uv_plane: &[u8],
    uv_stride: usize,
    width: usize,
    height: usize,
) -> Vec<u8> {
    let mut pixels = vec![0u8; width.saturating_mul(height).saturating_mul(4)];
    let uv_height = height / 2;
    for row in 0..height {
        if row * y_stride + width > y_plane.len() {
            break;
        }
        let uv_row = (row / 2).min(uv_height.saturating_sub(1));
        if uv_row * uv_stride + width > uv_plane.len() {
            break;
        }
        let y_row = &y_plane[row * y_stride..row * y_stride + width];
        let uv_row = &uv_plane[uv_row * uv_stride..uv_row * uv_stride + width];
        let out_row = &mut pixels[row * width * 4..(row + 1) * width * 4];
        for column in 0..width {
            let y = i32::from(y_row[column]) - 16;
            let uv_index = (column / 2) * 2;
            let u = i32::from(uv_row[uv_index]) - 128;
            let v = i32::from(uv_row[uv_index + 1]) - 128;
            let r = (1192 * y + 1836 * v + 512) >> 10;
            let g = (1192 * y - 218 * u - 546 * v + 512) >> 10;
            let b = (1192 * y + 2163 * u + 512) >> 10;
            let out = &mut out_row[column * 4..column * 4 + 4];
            out[0] = b.clamp(0, 255) as u8;
            out[1] = g.clamp(0, 255) as u8;
            out[2] = r.clamp(0, 255) as u8;
            out[3] = 255;
        }
    }
    pixels
}
