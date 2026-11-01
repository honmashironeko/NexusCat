use crate::error::{NexusError, Result};
use crate::ids::DeviceId;
use crate::mode::Mode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Zh,
    En,
}

impl Language {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoTarget {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

impl Default for VideoTarget {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 20_000,
        }
    }
}

impl VideoTarget {
    pub fn recommended_bitrate_kbps(&self) -> u32 {
        const BITS_PER_PIXEL: f64 = 0.2;
        const STEP_KBPS: f64 = 500.0;
        let raw =
            (self.width as f64 * self.height as f64 * self.fps as f64 * BITS_PER_PIXEL) / 1000.0;
        let rounded = ((raw / STEP_KBPS).round() as u32).saturating_mul(500);
        rounded.clamp(4_000, 100_000)
    }

    pub fn with_recommended_bitrate(mut self) -> Self {
        self.bitrate_kbps = self.recommended_bitrate_kbps();
        self
    }

    pub fn clamped_to(mut self, max_width: u32, max_height: u32, max_fps: u32) -> Self {
        let previous = (self.width, self.height, self.fps);
        self.width = self.width.min(max_width.max(2)) & !1;
        self.height = self.height.min(max_height.max(2)) & !1;
        self.fps = self.fps.min(max_fps.max(1));
        if previous != (self.width, self.height, self.fps) {
            self.bitrate_kbps = self.recommended_bitrate_kbps();
        }
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.width < 320 || self.height < 240 {
            return Err(NexusError::invalid("视频分辨率不能低于 320x240"));
        }
        if self.width > 7680 || self.height > 4320 {
            return Err(NexusError::invalid("视频分辨率不能超过 8K"));
        }
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(NexusError::invalid("H.264 要求宽高为偶数"));
        }
        if !(1..=240).contains(&self.fps) {
            return Err(NexusError::invalid("帧率必须在 1..=240 之间"));
        }
        if !(500..=200_000).contains(&self.bitrate_kbps) {
            return Err(NexusError::invalid("码率必须在 500..=200000 kbps 之间"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedConfig {
    pub language: Language,
    pub theme: Theme,
    pub audio_enabled: bool,
    pub clipboard_enabled: bool,
    pub update_auto_accept: bool,
    pub target: VideoTarget,
    #[serde(default)]
    pub video_targets: BTreeMap<DeviceId, VideoTarget>,
}

impl SharedConfig {
    pub fn validate(&self) -> Result<()> {
        self.target.validate()?;
        for target in self.video_targets.values() {
            target.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedConfigSnapshot {
    pub revision: u64,
    pub updated_at_ms: u64,
    pub source: DeviceId,
    pub config: SharedConfig,
}

impl SharedConfigSnapshot {
    pub fn is_newer_than(
        &self,
        revision: u64,
        updated_at_ms: u64,
        source: Option<DeviceId>,
    ) -> bool {
        if self.revision != revision {
            return self.revision > revision;
        }
        if self.updated_at_ms != updated_at_ms {
            return self.updated_at_ms > updated_at_ms;
        }
        source.is_none_or(|current| self.source > current)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutSettings {
    pub toggle_mode: String,
    pub toggle_stage: String,
    pub open_calibration: String,
    #[serde(default = "default_pause_remote_shortcut")]
    pub pause_remote: String,
    #[serde(default = "default_toggle_edge_shortcut")]
    pub toggle_edge: String,
}

fn default_pause_remote_shortcut() -> String {
    "Ctrl+Alt+Space".into()
}

fn default_toggle_edge_shortcut() -> String {
    "Ctrl+Alt+E".into()
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            toggle_mode: "Ctrl+Alt+M".into(),
            toggle_stage: "Ctrl+Alt+F".into(),
            open_calibration: "Ctrl+Alt+C".into(),
            pause_remote: default_pause_remote_shortcut(),
            toggle_edge: default_toggle_edge_shortcut(),
        }
    }
}

impl ShortcutSettings {
    pub fn validate(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for (label, value) in [
            ("模式切换", self.toggle_mode.as_str()),
            ("全屏", self.toggle_stage.as_str()),
            ("校准层", self.open_calibration.as_str()),
            ("暂停远端控制", self.pause_remote.as_str()),
            ("暂停边缘触发", self.toggle_edge.as_str()),
        ] {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if value.chars().count() > 64 {
                return Err(NexusError::invalid(format!("{label}快捷键过长")));
            }
            if !seen.insert(value.to_ascii_lowercase()) {
                return Err(NexusError::invalid("快捷键不能重复"));
            }
        }
        Ok(())
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    pub device_name: String,
    pub language: Language,
    pub theme: Theme,
    pub mode: Mode,
    #[serde(default = "default_true")]
    pub edge_enabled: bool,
    #[serde(default = "default_true")]
    pub edge_inset: bool,
    #[serde(default)]
    pub stage_on_primary: bool,
    #[serde(default)]
    pub shortcuts: ShortcutSettings,
    pub autostart: bool,
    pub audio_enabled: bool,
    pub clipboard_enabled: bool,
    pub file_transfer_enabled: bool,
    pub update_auto_accept: bool,
    pub target: VideoTarget,
    #[serde(default)]
    pub video_targets: BTreeMap<DeviceId, VideoTarget>,
    pub download_dir: Option<String>,
    pub control_port: u16,
    pub discovery_port: u16,
    #[serde(default)]
    pub shared_revision: u64,
    #[serde(default)]
    pub shared_updated_at_ms: u64,
    #[serde(default)]
    pub shared_source: Option<DeviceId>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            device_name: "NexusCat".into(),
            language: Language::Zh,
            theme: Theme::System,
            mode: Mode::Kvm,
            edge_enabled: true,
            edge_inset: true,
            stage_on_primary: false,
            shortcuts: ShortcutSettings::default(),
            autostart: true,
            audio_enabled: true,
            clipboard_enabled: true,
            file_transfer_enabled: true,
            update_auto_accept: false,
            target: VideoTarget::default(),
            video_targets: BTreeMap::new(),
            download_dir: None,
            control_port: 0,
            discovery_port: 47850,
            shared_revision: 0,
            shared_updated_at_ms: 0,
            shared_source: None,
        }
    }
}

impl AppSettings {
    pub fn validate(&self) -> Result<()> {
        let name = self.device_name.trim();
        if name.is_empty() {
            return Err(NexusError::invalid("设备名称不能为空"));
        }
        if name.chars().count() > 64 {
            return Err(NexusError::invalid("设备名称不能超过 64 个字符"));
        }
        self.shortcuts.validate()?;
        self.target.validate()?;
        for target in self.video_targets.values() {
            target.validate()?;
        }
        Ok(())
    }

    pub fn shared_config(&self) -> SharedConfig {
        SharedConfig {
            language: self.language,
            theme: self.theme,
            audio_enabled: self.audio_enabled,
            clipboard_enabled: self.clipboard_enabled,
            update_auto_accept: self.update_auto_accept,
            target: self.target,
            video_targets: self.video_targets.clone(),
        }
    }

    pub fn apply_shared_config(&mut self, config: &SharedConfig) {
        self.language = config.language;
        self.theme = config.theme;
        self.audio_enabled = config.audio_enabled;
        self.clipboard_enabled = config.clipboard_enabled;
        self.update_auto_accept = config.update_auto_accept;
        self.target = config.target;
        self.video_targets = config.video_targets.clone();
    }

    pub fn shared_snapshot(&self, source: DeviceId) -> SharedConfigSnapshot {
        SharedConfigSnapshot {
            revision: self.shared_revision,
            updated_at_ms: self.shared_updated_at_ms,
            source,
            config: self.shared_config(),
        }
    }

    pub fn bump_shared_config(
        &mut self,
        source: DeviceId,
        updated_at_ms: u64,
    ) -> SharedConfigSnapshot {
        self.shared_revision = self.shared_revision.saturating_add(1);
        self.shared_updated_at_ms = updated_at_ms;
        self.shared_source = Some(source);
        self.shared_snapshot(source)
    }

    pub fn apply_shared_snapshot(&mut self, snapshot: &SharedConfigSnapshot) -> bool {
        if !snapshot.is_newer_than(
            self.shared_revision,
            self.shared_updated_at_ms,
            self.shared_source,
        ) {
            return false;
        }
        self.apply_shared_config(&snapshot.config);
        self.shared_revision = snapshot.revision;
        self.shared_updated_at_ms = snapshot.updated_at_ms;
        self.shared_source = Some(snapshot.source);
        true
    }

    pub fn video_target_for(&self, device_id: DeviceId) -> VideoTarget {
        self.video_targets
            .get(&device_id)
            .copied()
            .unwrap_or(self.target)
    }
}

