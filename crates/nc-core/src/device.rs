use crate::ids::DeviceId;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    MacOs,
}

impl Platform {
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::Windows
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::MacOs => "macOS",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.display_name())
    }
}

impl FromStr for Platform {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.to_ascii_lowercase().as_str() {
            "windows" | "win" => Ok(Self::Windows),
            "macos" | "mac" | "darwin" => Ok(Self::MacOs),
            other => Err(format!("未知平台：{other}")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub screen_capture: bool,
    pub input_inject: bool,
    pub audio_capture: bool,
    pub clipboard: bool,
    pub file_transfer: bool,
    pub hardware_encode: bool,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
}

impl Capabilities {
    pub fn current() -> Self {
        Self {
            screen_capture: true,
            input_inject: true,
            audio_capture: true,
            clipboard: true,
            file_transfer: true,
            hardware_encode: false,
            max_width: 3840,
            max_height: 2160,
            max_fps: 60,
        }
    }

    pub fn clamped_to(&self, width: u32, height: u32, fps: u32) -> Self {
        let mut clamped = self.clone();
        clamped.max_width = clamped.max_width.min(width.max(1));
        clamped.max_height = clamped.max_height.min(height.max(1));
        clamped.max_fps = clamped.max_fps.min(fps.max(1));
        clamped
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    pub platform: Platform,
    pub capabilities: Capabilities,
    pub control_port: u16,
    pub media_port: u16,
}
