mod audio;
mod sck_screen;
mod screen;

use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Sender;
use nc_core::{NexusError, Result};
use tracing::warn;

use crate::{AudioCapturer, ScreenCapturer};

use self::audio::SckAudioCapturer;
use self::sck_screen::SckScreenCapturer;
use self::screen::CgScreenCapturer;

static PIXEL_BUFFER_DISABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn pixel_buffer_capture_disabled() -> bool {
    PIXEL_BUFFER_DISABLED.load(Ordering::Relaxed)
}

pub fn disable_pixel_buffer_capture() {
    PIXEL_BUFFER_DISABLED.store(true, Ordering::SeqCst);
}

pub(crate) struct MacScreenCapturer {
    sck: SckScreenCapturer,
    cg: CgScreenCapturer,
}

impl MacScreenCapturer {
    pub(crate) fn new() -> Self {
        Self {
            sck: SckScreenCapturer::new(),
            cg: CgScreenCapturer::new(),
        }
    }
}

impl ScreenCapturer for MacScreenCapturer {
    fn start(
        &mut self,
        config: crate::ScreenCaptureConfig,
        sink: Sender<crate::CapturedFrame>,
    ) -> Result<()> {
        if self.is_running() {
            return Err(NexusError::invalid("采集已在运行，不能重复启动"));
        }
        match self.sck.start(config, sink.clone()) {
            Ok(()) => Ok(()),
            Err(sck_error) => {
                warn!(%sck_error, "ScreenCaptureKit 屏幕采集不可用，回退到 CoreGraphics");
                self.cg.start(config, sink).map_err(|cg_error| {
                    NexusError::platform(format!(
                        "ScreenCaptureKit 失败：{sck_error}；CoreGraphics 回退失败：{cg_error}"
                    ))
                })
            }
        }
    }

    fn stop(&mut self) {
        self.sck.stop();
        self.cg.stop();
    }

    fn is_running(&self) -> bool {
        self.sck.is_running() || self.cg.is_running()
    }
}

pub(crate) fn create_screen_capturer() -> Result<Box<dyn ScreenCapturer>> {
    Ok(Box::new(MacScreenCapturer::new()))
}

pub(crate) fn create_audio_capturer() -> Result<Box<dyn AudioCapturer>> {
    Ok(Box::new(SckAudioCapturer::new()))
}
