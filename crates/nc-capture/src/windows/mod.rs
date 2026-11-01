mod audio;
pub(crate) mod gpu;
mod screen;
mod wgc;

pub(crate) use self::audio::WasapiAudioCapturer;
pub(crate) use self::screen::GdiScreenCapturer;
use self::wgc::WgcScreenCapturer;

use crossbeam_channel::Sender;
use nc_core::{NexusError, Result};
use tracing::warn;

use crate::{AudioCapturer, CapturedFrame, ScreenCaptureConfig, ScreenCapturer};

pub(crate) struct WindowsScreenCapturer {
    inner: Option<Box<dyn ScreenCapturer>>,
}

impl WindowsScreenCapturer {
    fn new() -> Self {
        Self { inner: None }
    }
}

impl ScreenCapturer for WindowsScreenCapturer {
    fn start(&mut self, config: ScreenCaptureConfig, sink: Sender<CapturedFrame>) -> Result<()> {
        if self.inner.is_some() {
            return Err(NexusError::invalid("采集已在运行，不能重复启动"));
        }
        let quarter_turned = nc_platform::monitors()
            .ok()
            .and_then(|monitors| {
                monitors
                    .into_iter()
                    .find(|monitor| monitor.id == config.monitor_id)
            })
            .is_some_and(|monitor| monitor.is_quarter_turned());
        let force_gdi = quarter_turned
            || std::env::var("NEXUSCAT_CAPTURE_BACKEND")
                .map(|value| value.eq_ignore_ascii_case("gdi"))
                .unwrap_or(false);
        if !force_gdi {
            let mut wgc = WgcScreenCapturer::new();
            match wgc.start(config, sink.clone()) {
                Ok(()) => {
                    self.inner = Some(Box::new(wgc));
                    return Ok(());
                }
                Err(error) => warn!(%error, "WGC 采集不可用，回退 GDI"),
            }
        }
        let mut gdi = GdiScreenCapturer::new();
        gdi.start(config, sink)?;
        self.inner = Some(Box::new(gdi));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut inner) = self.inner.take() {
            inner.stop();
        }
    }

    fn is_running(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|capturer| capturer.is_running())
    }
}

impl Drop for WindowsScreenCapturer {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) fn create_screen_capturer() -> Result<Box<dyn ScreenCapturer>> {
    Ok(Box::new(WindowsScreenCapturer::new()))
}

pub(crate) fn create_audio_capturer() -> Result<Box<dyn AudioCapturer>> {
    Ok(Box::new(WasapiAudioCapturer::new()))
}
