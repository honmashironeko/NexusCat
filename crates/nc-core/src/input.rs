use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1 << 0);
    pub const CONTROL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const META: Self = Self(1 << 3);
    pub const CAPS_LOCK: Self = Self(1 << 4);
    pub const NUM_LOCK: Self = Self(1 << 5);

    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn shift(self) -> bool {
        self.contains(Self::SHIFT)
    }

    pub const fn control(self) -> bool {
        self.contains(Self::CONTROL)
    }

    pub const fn alt(self) -> bool {
        self.contains(Self::ALT)
    }

    pub const fn meta(self) -> bool {
        self.contains(Self::META)
    }

    pub fn with_key(self, key_code: u32, pressed: bool) -> Self {
        let flag = match key_code {
            0xE1 | 0xE5 => Self::SHIFT,
            0xE0 | 0xE4 => Self::CONTROL,
            0xE2 | 0xE6 => Self::ALT,
            0xE3 | 0xE7 => Self::META,
            0x39 => Self::CAPS_LOCK,
            0x53 => Self::NUM_LOCK,
            _ => return self,
        };
        if pressed {
            self.union(flag)
        } else {
            Self(self.0 & !flag.0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButtonState {
    Down,
    Up,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEvent {
    Key {
        state: KeyState,
        key_code: u32,
        scan_code: u32,
        extended: bool,
        modifiers: Modifiers,
    },
    MouseMove {
        x: i32,
        y: i32,
        delta_x: i32,
        delta_y: i32,
        modifiers: Modifiers,
    },
    MouseButton {
        state: MouseButtonState,
        button: MouseButton,
        x: i32,
        y: i32,
        modifiers: Modifiers,
    },
    MouseWheel {
        delta_x: i32,
        delta_y: i32,
        x: i32,
        y: i32,
        modifiers: Modifiers,
    },
    ReleaseAll,
}

impl fmt::Display for InputEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key {
                state, key_code, ..
            } => write!(formatter, "key {state:?} {key_code:#x}"),
            Self::MouseMove { x, y, .. } => write!(formatter, "mouse move {x},{y}"),
            Self::MouseButton { state, button, .. } => {
                write!(formatter, "mouse {button:?} {state:?}")
            }
            Self::MouseWheel {
                delta_x, delta_y, ..
            } => {
                write!(formatter, "wheel {delta_x},{delta_y}")
            }
            Self::ReleaseAll => formatter.write_str("release all"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputEnvelope {
    pub seq: u64,
    pub timestamp_us: u64,
    pub event: InputEvent,
}
