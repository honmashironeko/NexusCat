use nc_core::{DeviceInfo, NexusError, Result};
use nc_crypto::{DeviceCertificate, Identity, TrustStore, TrustedDevice};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPurpose {
    Group,
    Session,
    Update,
}

impl ConnectionPurpose {
    pub(crate) const fn link_purpose(self) -> nc_proto::LinkPurpose {
        match self {
            Self::Group | Self::Session => nc_proto::LinkPurpose::Group,
            Self::Update => nc_proto::LinkPurpose::Update,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingIntent {
    Trusted,
    Pair,
}

#[derive(Clone)]
pub struct ControlConfig {
    pub identity: Arc<Identity>,
    pub local_device: DeviceInfo,
    pub certificate: Arc<DeviceCertificate>,
    pub trust_store: Arc<RwLock<TrustStore>>,
    pub trust_path: Option<PathBuf>,
    pub connect_timeout: Duration,
    pub handshake_timeout: Duration,
    pub heartbeat_interval: Duration,
    pub heartbeat_timeout: Duration,
}

impl ControlConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        for (name, duration) in [
            ("connect_timeout", self.connect_timeout),
            ("handshake_timeout", self.handshake_timeout),
            ("heartbeat_interval", self.heartbeat_interval),
            ("heartbeat_timeout", self.heartbeat_timeout),
        ] {
            if duration.is_zero() {
                return Err(NexusError::InvalidInput(format!("{name} 不能为零")));
            }
        }
        if self.local_device.id != self.identity.device_id() {
            return Err(NexusError::InvalidInput(
                "local_device.id 与本机身份不一致".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn local_device_info(&self) -> DeviceInfo {
        self.local_device.clone()
    }

    pub(crate) fn pinned_tls_fingerprint(&self, intent: PairingIntent) -> Result<Option<[u8; 32]>> {
        if intent != PairingIntent::Trusted {
            return Ok(None);
        }
        let store = self
            .trust_store
            .read()
            .map_err(|_| NexusError::Internal("信任存储锁已损坏".into()))?;
        let devices = store.all();
        if devices.len() == 1 {
            let fingerprint = devices[0].certificate_fingerprint;
            if fingerprint != [0u8; 32] {
                return Ok(Some(fingerprint));
            }
        }
        Ok(None)
    }

    pub(crate) fn trusted_record(
        &self,
        device_id: &nc_core::DeviceId,
    ) -> Result<Option<TrustedDevice>> {
        let store = self
            .trust_store
            .read()
            .map_err(|_| NexusError::Internal("信任存储锁已损坏".into()))?;
        Ok(store.get(device_id).cloned())
    }

    pub(crate) fn is_trusted(
        &self,
        device_id: &nc_core::DeviceId,
        static_public: &[u8; 32],
    ) -> Result<bool> {
        let store = self
            .trust_store
            .read()
            .map_err(|_| NexusError::Internal("信任存储锁已损坏".into()))?;
        Ok(store.is_trusted(device_id, static_public))
    }

    pub(crate) fn store_peer(
        &self,
        peer: &DeviceInfo,
        static_public: [u8; 32],
        certificate_fingerprint: [u8; 32],
    ) -> Result<()> {
        let record = TrustedDevice {
            device_id: peer.id,
            name: peer.name.clone(),
            platform: peer.platform,
            static_public,
            certificate_fingerprint,
            paired_at_ms: now_ms(),
        };
        let mut store = self
            .trust_store
            .write()
            .map_err(|_| NexusError::Internal("信任存储锁已损坏".into()))?;
        store.upsert(record);
        if let Some(path) = &self.trust_path {
            store
                .save(path)
                .map_err(|error| NexusError::Crypto(error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn update_certificate_fingerprint(
        &self,
        device_id: &nc_core::DeviceId,
        certificate_fingerprint: [u8; 32],
    ) -> Result<()> {
        let mut store = self
            .trust_store
            .write()
            .map_err(|_| NexusError::Internal("信任存储锁已损坏".into()))?;
        let mut record = store
            .get(device_id)
            .cloned()
            .ok_or_else(|| NexusError::Business("信任记录在握手期间被移除，停止连接".into()))?;
        if record.certificate_fingerprint == certificate_fingerprint {
            return Ok(());
        }
        record.certificate_fingerprint = certificate_fingerprint;
        store.upsert(record);
        if let Some(path) = &self.trust_path {
            store
                .save(path)
                .map_err(|error| NexusError::Crypto(error.to_string()))?;
        }
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}
