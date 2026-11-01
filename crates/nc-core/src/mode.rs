use crate::ids::DeviceId;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Kvm,
    Aggregation,
}

impl Mode {
    pub const fn uses_media(self) -> bool {
        matches!(self, Self::Aggregation)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kvm => "kvm",
            Self::Aggregation => "aggregation",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Mode {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.to_ascii_lowercase().as_str() {
            "kvm" | "flow" | "multi_screen_flow" => Ok(Self::Kvm),
            "aggregation" | "stage" | "single_screen" => Ok(Self::Aggregation),
            other => Err(format!("未知模式：{other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiState {
    #[default]
    Background,
    ControlPanel,
    Stage,
}

impl UiState {
    pub const fn is_window_visible(self) -> bool {
        !matches!(self, Self::Background)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::ControlPanel => "control_panel",
            Self::Stage => "stage",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InputOwner {
    #[default]
    Local,
    Remote { device_id: DeviceId },
}

impl InputOwner {
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    pub fn device_id(&self) -> Option<DeviceId> {
        match self {
            Self::Local => None,
            Self::Remote { device_id } => Some(*device_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }

    pub const fn vector(self) -> (i32, i32) {
        match self {
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
            Self::Up => (0, -1),
            Self::Down => (0, 1),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

