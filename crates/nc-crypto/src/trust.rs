use crate::error::{CryptoError, Result};
use nc_core::{DeviceId, Platform};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub device_id: DeviceId,
    pub name: String,
    pub platform: Platform,
    pub static_public: [u8; 32],
    pub certificate_fingerprint: [u8; 32],
    pub paired_at_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct TrustFile {
    version: u32,
    devices: Vec<TrustedDevice>,
}

const TRUST_FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default)]
pub struct TrustStore {
    devices: HashMap<DeviceId, TrustedDevice>,
}

impl TrustStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let bytes = std::fs::read(path)?;
        let file: TrustFile = serde_json::from_slice(&bytes)?;
        if file.version != TRUST_FILE_VERSION {
            return Err(CryptoError::TrustStore(format!(
                "不支持的信任存储版本 {}",
                file.version
            )));
        }
        let mut devices = HashMap::with_capacity(file.devices.len());
        for device in file.devices {
            devices.insert(device.device_id, device);
        }
        Ok(Self { devices })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut devices: Vec<_> = self.devices.values().cloned().collect();
        devices.sort_by_key(|device| device.device_id);
        let file = TrustFile {
            version: TRUST_FILE_VERSION,
            devices,
        };
        let json = serde_json::to_vec_pretty(&file)?;
        let temp_path = path.with_extension("json.tmp");
        std::fs::write(&temp_path, json)?;
        std::fs::rename(&temp_path, path)?;
        Ok(())
    }

    pub fn upsert(&mut self, device: TrustedDevice) {
        self.devices.insert(device.device_id, device);
    }

    pub fn remove(&mut self, device_id: &DeviceId) -> bool {
        self.devices.remove(device_id).is_some()
    }

    pub fn get(&self, device_id: &DeviceId) -> Option<&TrustedDevice> {
        self.devices.get(device_id)
    }

    pub fn is_trusted(&self, device_id: &DeviceId, static_public: &[u8; 32]) -> bool {
        self.devices
            .get(device_id)
            .map(|device| bool::from(device.static_public.ct_eq(static_public)))
            .unwrap_or(false)
    }

    pub fn all(&self) -> Vec<TrustedDevice> {
        let mut devices: Vec<_> = self.devices.values().cloned().collect();
        devices.sort_by_key(|device| device.device_id);
        devices
    }

    pub fn len(&self) -> usize {
        self.devices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }
}
