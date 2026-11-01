use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    ScreenRecording,
    Accessibility,
    InputMonitoring,
}

impl PermissionKind {
    pub const fn required_for_platform() -> &'static [Self] {
        #[cfg(target_os = "macos")]
        {
            &[
                Self::ScreenRecording,
                Self::Accessibility,
                Self::InputMonitoring,
            ]
        }
        #[cfg(not(target_os = "macos"))]
        {
            &[]
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ScreenRecording => "screen_recording",
            Self::Accessibility => "accessibility",
            Self::InputMonitoring => "input_monitoring",
        }
    }
}

impl fmt::Display for PermissionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionStatus {
    pub kind: PermissionKind,
    pub granted: bool,
}

impl PermissionStatus {
    pub const fn new(kind: PermissionKind, granted: bool) -> Self {
        Self { kind, granted }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PermissionReport {
    pub statuses: Vec<PermissionStatus>,
}

impl PermissionReport {
    pub fn all_granted(&self) -> bool {
        self.statuses.iter().all(|status| status.granted)
    }

    pub fn missing(&self) -> Vec<PermissionKind> {
        self.statuses
            .iter()
            .filter(|status| !status.granted)
            .map(|status| status.kind)
            .collect()
    }

    pub fn get(&self, kind: PermissionKind) -> Option<bool> {
        self.statuses
            .iter()
            .find(|status| status.kind == kind)
            .map(|status| status.granted)
    }
}
