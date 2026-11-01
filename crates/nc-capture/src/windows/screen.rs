use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;

use crossbeam_channel::Sender;
use nc_core::{NexusError, PixelFormat, Result, VideoFrame};
use nc_platform::RectI;
use tracing::debug;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
    COLORONCOLOR, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, ROP_CODE, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, ICONINFO,
};

use crate::frame_loop::run_frame_loop;
use crate::worker::CaptureWorker;
use crate::{CapturedFrame, ScreenCaptureConfig, ScreenCapturer, STARTUP_TIMEOUT};

const MAX_FPS: u32 = 240;

pub(crate) struct GdiScreenCapturer {
    worker: CaptureWorker,
}

impl GdiScreenCapturer {
    pub(crate) fn new() -> Self {
        Self {
            worker: CaptureWorker::new(),
        }
    }
}

impl ScreenCapturer for GdiScreenCapturer {
    fn start(&mut self, config: ScreenCaptureConfig, sink: Sender<CapturedFrame>) -> Result<()> {
        validate_config(&config)?;
        tracing::info!(
            target_width = config.target_width,
            target_height = config.target_height,
            "启动 GDI 采集"
        );
        let monitors = nc_platform::monitors()?;
        let monitor = monitors
            .iter()
            .find(|monitor| monitor.id == config.monitor_id)
            .ok_or_else(|| NexusError::NotFound(format!("未找到显示器 {}", config.monitor_id)))?;
        if monitor.bounds.width == 0 || monitor.bounds.height == 0 {
            return Err(NexusError::platform(format!(
                "显示器 {} 的尺寸为 0",
                config.monitor_id
            )));
        }
        let bounds = monitor.bounds;
        let target_width = if config.target_width == 0 {
            bounds.width
        } else {
            config.target_width.min(bounds.width)
        } & !1;
        let target_height = if config.target_height == 0 {
            bounds.height
        } else {
            config.target_height.min(bounds.height)
        } & !1;
        if target_width < 2 || target_height < 2 {
            return Err(NexusError::invalid("采集目标尺寸不能小于 2x2"));
        }

        self.worker.spawn(
            "nc-capture-gdi",
            STARTUP_TIMEOUT,
            move |running, startup| match GdiResources::new(bounds, target_width, target_height) {
                Ok(mut resources) => {
                    let _ = startup.send(Ok(()));
                    let mut stats_since = std::time::Instant::now();
                    let mut stats_frames = 0u64;
                    let mut stats_capture_us = 0u128;
                    run_frame_loop(config.fps, &sink, &running, || {
                        let started = std::time::Instant::now();
                        let result = resources.capture(config.include_cursor, config.monitor_id);
                        stats_frames += 1;
                        stats_capture_us += started.elapsed().as_micros();
                        if stats_since.elapsed() >= std::time::Duration::from_secs(5) {
                            let elapsed = stats_since.elapsed().as_secs_f64();
                            tracing::info!(
                                fps = stats_frames as f64 / elapsed.max(0.001),
                                capture_ms =
                                    stats_capture_us as f64 / 1000.0 / stats_frames.max(1) as f64,
                                "GDI 采集统计"
                            );
                            stats_since = std::time::Instant::now();
                            stats_frames = 0;
                            stats_capture_us = 0;
                        }
                        result
                    });
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

struct GdiResources {
    screen_dc: HDC,
    memory_dc: HDC,
    bitmap: HBITMAP,
    previous_bitmap: HGDIOBJ,
    bits: *mut u8,
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    origin_x: i32,
    origin_y: i32,
}

impl GdiResources {
    fn new(bounds: RectI, target_width: u32, target_height: u32) -> Result<Self> {
        let width = i32::try_from(target_width)
            .map_err(|_| NexusError::invalid("采集目标宽度超出 GDI 支持范围"))?;
        let height = i32::try_from(target_height)
            .map_err(|_| NexusError::invalid("采集目标高度超出 GDI 支持范围"))?;

        unsafe {
            let screen_dc = GetDC(None);
            if screen_dc.is_invalid() {
                return Err(NexusError::platform("GetDC(NULL) 失败"));
            }

            let memory_dc = CreateCompatibleDC(Some(screen_dc));
            if memory_dc.is_invalid() {
                let _ = ReleaseDC(None, screen_dc);
                return Err(NexusError::platform("CreateCompatibleDC 失败"));
            }

            let bitmap_info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = ptr::null_mut();
            let bitmap = match CreateDIBSection(
                Some(screen_dc),
                &bitmap_info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            ) {
                Ok(bitmap) => bitmap,
                Err(error) => {
                    let _ = DeleteDC(memory_dc);
                    let _ = ReleaseDC(None, screen_dc);
                    return Err(NexusError::platform(format!(
                        "CreateDIBSection 失败：{error}"
                    )));
                }
            };
            if bitmap.is_invalid() || bits.is_null() {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(memory_dc);
                let _ = ReleaseDC(None, screen_dc);
                return Err(NexusError::platform("CreateDIBSection 返回空位图"));
            }

            let previous_bitmap = SelectObject(memory_dc, bitmap.into());
            if previous_bitmap.is_invalid() {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(memory_dc);
                let _ = ReleaseDC(None, screen_dc);
                return Err(NexusError::platform("SelectObject 选择 DIB 位图失败"));
            }
            SetStretchBltMode(memory_dc, COLORONCOLOR);

            Ok(Self {
                screen_dc,
                memory_dc,
                bitmap,
                previous_bitmap,
                bits: bits.cast(),
                width: target_width,
                height: target_height,
                source_width: bounds.width,
                source_height: bounds.height,
                origin_x: bounds.x,
                origin_y: bounds.y,
            })
        }
    }

    fn capture(&mut self, include_cursor: bool, monitor_id: u32) -> Result<CapturedFrame> {
        unsafe {
            if self.width == self.source_width && self.height == self.source_height {
                BitBlt(
                    self.memory_dc,
                    0,
                    0,
                    self.width as i32,
                    self.height as i32,
                    Some(self.screen_dc),
                    self.origin_x,
                    self.origin_y,
                    ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0),
                )
                .map_err(|error| NexusError::platform(format!("BitBlt 失败：{error}")))?;
            } else if !StretchBlt(
                self.memory_dc,
                0,
                0,
                self.width as i32,
                self.height as i32,
                Some(self.screen_dc),
                self.origin_x,
                self.origin_y,
                self.source_width as i32,
                self.source_height as i32,
                ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0),
            )
            .as_bool()
            {
                return Err(NexusError::platform("StretchBlt 失败"));
            }
        }

        if include_cursor {
            let scale_x = self.width as f64 / self.source_width as f64;
            let scale_y = self.height as f64 / self.source_height as f64;
            draw_cursor(
                self.memory_dc,
                self.origin_x as f64,
                self.origin_y as f64,
                scale_x,
                scale_y,
            );
        }

        let stride = self.width as usize * 4;
        let data_len = stride * self.height as usize;
        let data = unsafe { std::slice::from_raw_parts(self.bits, data_len) }.to_vec();
        let timestamp_us = crate::unix_time_us();

        Ok(CapturedFrame {
            frame: VideoFrame {
                width: self.width,
                height: self.height,
                stride,
                format: PixelFormat::Bgra8,
                timestamp_us,
                data,
            },
            gpu: None,
            captured_at_us: timestamp_us,
            monitor_id,
        })
    }
}

impl Drop for GdiResources {
    fn drop(&mut self) {
        unsafe {
            if !self.previous_bitmap.is_invalid() {
                let _ = SelectObject(self.memory_dc, self.previous_bitmap);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
            if !self.memory_dc.is_invalid() {
                let _ = DeleteDC(self.memory_dc);
            }
            if !self.screen_dc.is_invalid() {
                let _ = ReleaseDC(None, self.screen_dc);
            }
        }
    }
}

fn draw_cursor(memory_dc: HDC, origin_x: f64, origin_y: f64, scale_x: f64, scale_y: f64) {
    let mut cursor_info = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    if let Err(error) = unsafe { GetCursorInfo(&mut cursor_info) } {
        debug!(%error, "GetCursorInfo 失败，跳过光标");
        return;
    }
    if cursor_info.flags.0 & CURSOR_SHOWING.0 == 0 || cursor_info.hCursor.is_invalid() {
        return;
    }

    let mut hotspot_x = 0;
    let mut hotspot_y = 0;
    let mut icon_info = ICONINFO::default();
    match unsafe { GetIconInfo(cursor_info.hCursor.into(), &mut icon_info) } {
        Ok(()) => {
            hotspot_x = icon_info.xHotspot as i32;
            hotspot_y = icon_info.yHotspot as i32;
            delete_icon_bitmap(icon_info.hbmMask);
            delete_icon_bitmap(icon_info.hbmColor);
        }
        Err(error) => debug!(%error, "GetIconInfo 失败，按原始光标坐标绘制"),
    }

    let target_x = ((f64::from(cursor_info.ptScreenPos.x) - origin_x) * scale_x
        - f64::from(hotspot_x))
    .round() as i32;
    let target_y = ((f64::from(cursor_info.ptScreenPos.y) - origin_y) * scale_y
        - f64::from(hotspot_y))
    .round() as i32;
    if let Err(error) = unsafe {
        DrawIconEx(
            memory_dc,
            target_x,
            target_y,
            cursor_info.hCursor.into(),
            0,
            0,
            0,
            None,
            DI_NORMAL,
        )
    } {
        debug!(%error, "DrawIconEx 失败，跳过光标");
    }
}

fn delete_icon_bitmap(bitmap: HBITMAP) {
    if !bitmap.is_invalid() {
        unsafe {
            let _ = DeleteObject(bitmap.into());
        }
    }
}
