use nc_core::{Capabilities, DeviceId, DeviceInfo, NexusError, Platform};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_DISCOVERY_PORT: u16 = 47850;
const DEFAULT_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_DEVICE_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_DEVICE_NAME: &str = "NexusCat";
const MAX_DEVICE_NAME_CHARS: usize = 64;
const DEVICE_ID_ENV: &str = "NEXUSCAT_DEVICE_ID";
const DEVICE_NAME_ENV: &str = "NEXUSCAT_DEVICE_NAME";
const CONTROL_PORT_ENV: &str = "NEXUSCAT_CONTROL_PORT";
const MEDIA_PORT_ENV: &str = "NEXUSCAT_MEDIA_PORT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryConfig {
    pub discovery_port: u16,
    pub announce_interval: Duration,
    pub device_timeout: Duration,
    pub extra_targets: Vec<SocketAddr>,
    pub interface_broadcast: bool,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            discovery_port: DEFAULT_DISCOVERY_PORT,
            announce_interval: DEFAULT_ANNOUNCE_INTERVAL,
            device_timeout: DEFAULT_DEVICE_TIMEOUT,
            extra_targets: Vec::new(),
            interface_broadcast: true,
        }
    }
}

impl DiscoveryConfig {
    pub(crate) fn validate(&self) -> Result<(), NexusError> {
        if self.announce_interval.is_zero() {
            return Err(NexusError::invalid("发现通告间隔不能为 0"));
        }
        if self.device_timeout.is_zero() {
            return Err(NexusError::invalid("发现设备超时不能为 0"));
        }
        if let Some(target) = self.extra_targets.iter().find(|target| target.is_ipv6()) {
            return Err(NexusError::invalid(format!(
                "发现目标仅支持 IPv4：{target}"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    pub info: DeviceInfo,
    pub address: SocketAddr,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    Found(DiscoveredDevice),
    Updated(DiscoveredDevice),
    Lost(DeviceId),
}

pub(crate) fn discoverable_local_info() -> DeviceInfo {
    DeviceInfo {
        id: local_device_id(),
        name: local_device_name(),
        platform: Platform::current(),
        capabilities: Capabilities::current(),
        control_port: env_port(CONTROL_PORT_ENV),
        media_port: env_port(MEDIA_PORT_ENV),
    }
}

fn local_device_id() -> DeviceId {
    match std::env::var(DEVICE_ID_ENV) {
        Ok(text) => match DeviceId::from_hex(text.trim()) {
            Ok(id) => id,
            Err(error) => {
                tracing::warn!(%error, "NEXUSCAT_DEVICE_ID 无效，改用临时设备 ID");
                random_device_id()
            }
        },
        Err(_) => random_device_id(),
    }
}

fn random_device_id() -> DeviceId {
    static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);

    let counter = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let process = std::process::id();

    let mut first = RandomState::new().build_hasher();
    first.write_u64(counter);
    first.write_u32(process);
    first.write_u128(timestamp);

    let mut second = RandomState::new().build_hasher();
    second.write_u64(!counter);
    second.write_u32(process);
    second.write_u128(timestamp);

    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&first.finish().to_le_bytes());
    bytes[8..].copy_from_slice(&second.finish().to_le_bytes());
    DeviceId::from_bytes(bytes)
}

fn local_device_name() -> String {
    let configured = std::env::var(DEVICE_NAME_ENV)
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    let name = configured
        .or_else(host_name)
        .unwrap_or_else(|| DEFAULT_DEVICE_NAME.to_owned());
    truncate_chars(name, MAX_DEVICE_NAME_CHARS)
}

fn host_name() -> Option<String> {
    ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

fn truncate_chars(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    text.chars().take(max_chars).collect()
}

fn env_port(key: &str) -> u16 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .unwrap_or(0)
}
