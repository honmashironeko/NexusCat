use nc_core::{NexusError, Result};

use crate::{AudioCapturer, ScreenCapturer};

pub(crate) fn create_screen_capturer() -> Result<Box<dyn ScreenCapturer>> {
    Err(NexusError::platform("当前平台不支持屏幕采集"))
}

pub(crate) fn create_audio_capturer() -> Result<Box<dyn AudioCapturer>> {
    Err(NexusError::platform("当前平台不支持系统音频回环采集"))
}
