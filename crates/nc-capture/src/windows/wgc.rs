use crossbeam_channel::{bounded, Sender};
use nc_core::{NexusError, PixelFormat, Result, VideoFrame};
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl};
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use super::gpu::{gpu_capture_enabled, gpu_capture_suppressed, GpuFrame, GpuScaler};
use crate::{CapturedFrame, ScreenCaptureConfig, ScreenCapturer};

type WgcControl = CaptureControl<WgcHandler, WgcHandlerError>;

pub(crate) struct WgcScreenCapturer {
    control: Option<WgcControl>,
}

impl WgcScreenCapturer {
    pub(crate) fn new() -> Self {
        Self { control: None }
    }
}

impl ScreenCapturer for WgcScreenCapturer {
    fn start(&mut self, config: ScreenCaptureConfig, sink: Sender<CapturedFrame>) -> Result<()> {
        if self
            .control
            .as_ref()
            .is_some_and(|control| !control.is_finished())
        {
            return Err(NexusError::invalid("采集已在运行，不能重复启动"));
        }
        self.control = None;
        if config.fps == 0 || config.fps > 240 {
            return Err(NexusError::invalid("屏幕采集 fps 必须在 1..=240 之间"));
        }
        let monitor = select_monitor(config.monitor_id)?;
        let interval = std::time::Duration::from_nanos(1_000_000_000 / u64::from(config.fps));
        tracing::info!(
            gpu = super::gpu::gpu_capture_enabled(),
            target_width = config.target_width,
            target_height = config.target_height,
            "启动 WGC 采集"
        );
        let (first_frame_tx, first_frame_rx) = bounded(1);
        let flags = WgcFlags {
            sink,
            monitor_id: config.monitor_id,
            first_frame: first_frame_tx,
            target_width: config.target_width,
            target_height: config.target_height,
            fps: config.fps,
        };
        let minimum_update_interval =
            match GraphicsCaptureApi::is_minimum_update_interval_supported() {
                Ok(true) => MinimumUpdateIntervalSettings::Custom(interval),
                Ok(false) => {
                    tracing::warn!(
                    fps = config.fps,
                    "当前系统不支持 WGC MinUpdateInterval，按系统默认节奏采集（高帧率可能被限制）"
                );
                    MinimumUpdateIntervalSettings::Default
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        fps = config.fps,
                        "检查 WGC MinUpdateInterval 失败，按系统默认节奏采集"
                    );
                    MinimumUpdateIntervalSettings::Default
                }
            };
        let settings = Settings::new(
            monitor,
            if config.include_cursor {
                CursorCaptureSettings::WithCursor
            } else {
                CursorCaptureSettings::WithoutCursor
            },
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            minimum_update_interval,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            flags,
        );
        let control = WgcHandler::start_free_threaded(settings)
            .map_err(|error| NexusError::platform(format!("WGC 采集启动失败：{error}")))?;
        if first_frame_rx
            .recv_timeout(std::time::Duration::from_millis(400))
            .is_err()
        {
            let _ = control.stop();
            return Err(NexusError::platform("WGC 未产生首帧"));
        }
        self.control = Some(control);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(control) = self.control.take() {
            let _ = control.stop();
        }
    }

    fn is_running(&self) -> bool {
        self.control
            .as_ref()
            .is_some_and(|control| !control.is_finished())
    }
}

impl Drop for WgcScreenCapturer {
    fn drop(&mut self) {
        self.stop();
    }
}

struct WgcFlags {
    sink: Sender<CapturedFrame>,
    monitor_id: u32,
    first_frame: Sender<()>,
    target_width: u32,
    target_height: u32,
    fps: u32,
}

struct WgcHandler {
    sink: Sender<CapturedFrame>,
    monitor_id: u32,
    first_frame: Sender<()>,
    announced: bool,
    target_width: u32,
    target_height: u32,
    fps: u32,
    gpu_enabled: bool,
    gpu_failed: bool,
    gpu_source: Option<(u32, u32)>,
    gpu_scaler: Option<GpuScaler>,
    stats_since: std::time::Instant,
    stats_frames: u64,
    stats_callback_us: u128,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WgcHandlerError {
    #[error("{0}")]
    Message(String),
}

impl GraphicsCaptureApiHandler for WgcHandler {
    type Flags = WgcFlags;
    type Error = WgcHandlerError;

    fn new(ctx: Context<Self::Flags>) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            sink: ctx.flags.sink,
            monitor_id: ctx.flags.monitor_id,
            first_frame: ctx.flags.first_frame,
            announced: false,
            target_width: ctx.flags.target_width,
            target_height: ctx.flags.target_height,
            fps: ctx.flags.fps,
            gpu_enabled: gpu_capture_enabled(),
            gpu_failed: false,
            gpu_source: None,
            gpu_scaler: None,
            stats_since: std::time::Instant::now(),
            stats_frames: 0,
            stats_callback_us: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> std::result::Result<(), Self::Error> {
        if frame.width() < 2 || frame.height() < 2 {
            return Ok(());
        }
        let callback_started = std::time::Instant::now();
        let captured = match self.try_gpu_frame(frame) {
            Some(captured) => captured,
            None => self.capture_cpu_frame(frame)?,
        };
        self.stats_frames += 1;
        self.stats_callback_us += callback_started.elapsed().as_micros();
        if self.stats_since.elapsed() >= std::time::Duration::from_secs(5) {
            let elapsed = self.stats_since.elapsed().as_secs_f64();
            let fps = self.stats_frames as f64 / elapsed.max(0.001);
            let callback_ms =
                self.stats_callback_us as f64 / 1000.0 / self.stats_frames.max(1) as f64;
            tracing::info!(
                fps = fps,
                callback_ms = callback_ms,
                gpu = self.gpu_enabled && !self.gpu_failed,
                "WGC 采集统计"
            );
            self.stats_since = std::time::Instant::now();
            self.stats_frames = 0;
            self.stats_callback_us = 0;
        }
        if !self.announced {
            let _ = self.first_frame.try_send(());
            self.announced = true;
        }
        match self.sink.try_send(captured) {
            Ok(()) => {}
            Err(crossbeam_channel::TrySendError::Full(_)) => {}
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                capture_control.stop();
            }
        }
        Ok(())
    }
}

impl WgcHandler {
    fn try_gpu_frame(&mut self, frame: &Frame) -> Option<CapturedFrame> {
        if !self.gpu_enabled || self.gpu_failed {
            return None;
        }
        if gpu_capture_suppressed() {
            tracing::warn!("GPU 采集已在运行时禁用，WGC 回退 CPU 采集");
            self.gpu_enabled = false;
            self.gpu_scaler = None;
            self.gpu_source = None;
            return None;
        }
        match self.build_gpu_frame(frame) {
            Ok(captured) => Some(captured),
            Err(error) => {
                tracing::warn!(%error, "GPU 采集路径不可用，回退 CPU");
                self.gpu_failed = true;
                self.gpu_scaler = None;
                self.gpu_source = None;
                None
            }
        }
    }

    fn build_gpu_frame(&mut self, frame: &Frame) -> std::result::Result<CapturedFrame, String> {
        let source = (frame.width(), frame.height());
        let target = self.gpu_target_size(source);
        let texture = frame.as_raw_texture();
        let device = unsafe { texture.GetDevice() }
            .map_err(|error| format!("读取 WGC 设备失败：{error}"))?;
        if self.gpu_scaler.is_none() || self.gpu_source != Some(source) {
            self.gpu_scaler = Some(
                GpuScaler::new(&device, source.0, source.1, target.0, target.1, self.fps)
                    .map_err(|error| format!("创建 GPU 缩放器失败：{error}"))?,
            );
            self.gpu_source = Some(source);
        }
        let nv12 = self
            .gpu_scaler
            .as_ref()
            .expect("GPU 缩放器已创建")
            .scale(texture)
            .map_err(|error| format!("GPU 缩放失败：{error}"))?;
        let timestamp_us = crate::unix_time_us();
        Ok(CapturedFrame {
            frame: VideoFrame {
                width: target.0,
                height: target.1,
                stride: target.0 as usize * 4,
                format: PixelFormat::Bgra8,
                timestamp_us,
                data: Vec::new(),
            },
            gpu: Some(GpuFrame {
                device,
                texture: nv12,
                width: target.0,
                height: target.1,
            }),
            captured_at_us: timestamp_us,
            monitor_id: self.monitor_id,
        })
    }

    fn gpu_target_size(&self, source: (u32, u32)) -> (u32, u32) {
        let width = if self.target_width >= 2 {
            self.target_width
        } else {
            source.0
        };
        let height = if self.target_height >= 2 {
            self.target_height
        } else {
            source.1
        };
        (width.max(2) & !1, height.max(2) & !1)
    }

    fn capture_cpu_frame(
        &mut self,
        frame: &mut Frame,
    ) -> std::result::Result<CapturedFrame, WgcHandlerError> {
        let width = frame.width();
        let height = frame.height();
        let (pixels, pixel_format) = {
            let buffer = frame
                .buffer()
                .map_err(|error| WgcHandlerError::Message(format!("读取 WGC 帧失败：{error}")))?;
            let format = match buffer.color_format() {
                ColorFormat::Rgba8 => PixelFormat::Rgba8,
                ColorFormat::Bgra8 => PixelFormat::Bgra8,
                ColorFormat::Rgba16F => {
                    return Err(WgcHandlerError::Message(
                        "WGC 返回不支持的 RGBA16F 帧".into(),
                    ))
                }
            };
            let mut contiguous = Vec::new();
            let data = buffer.as_nopadding_buffer(&mut contiguous).to_vec();
            (data, format)
        };
        let timestamp_us = crate::unix_time_us();
        Ok(CapturedFrame {
            frame: VideoFrame {
                width,
                height,
                stride: width as usize * 4,
                format: pixel_format,
                timestamp_us,
                data: pixels,
            },
            gpu: None,
            captured_at_us: timestamp_us,
            monitor_id: self.monitor_id,
        })
    }
}

fn select_monitor(monitor_id: u32) -> Result<Monitor> {
    let platform_monitors = nc_platform::monitors()
        .map_err(|error| NexusError::platform(format!("枚举显示器失败：{error}")))?;
    let target = platform_monitors
        .iter()
        .find(|monitor| monitor.id == monitor_id)
        .ok_or_else(|| NexusError::NotFound(format!("未找到显示器 {monitor_id}")))?;
    let monitors = Monitor::enumerate()
        .map_err(|error| NexusError::platform(format!("枚举 WGC 显示器失败：{error}")))?;
    let by_name = monitors.iter().find(|monitor| {
        let name = monitor.name().ok();
        name.as_deref() == Some(target.name.as_str())
            && monitor.width().ok() == Some(target.bounds.width)
            && monitor.height().ok() == Some(target.bounds.height)
    });
    let by_size = monitors.iter().find(|monitor| {
        monitor.width().ok() == Some(target.bounds.width)
            && monitor.height().ok() == Some(target.bounds.height)
    });
    if let Some(monitor) = by_name.or(by_size) {
        return Ok(*monitor);
    }
    if target.primary {
        if let Ok(primary) = Monitor::primary() {
            return Ok(primary);
        }
    }
    monitors
        .into_iter()
        .next()
        .ok_or_else(|| NexusError::NotFound("WGC 未返回可用显示器".into()))
}
