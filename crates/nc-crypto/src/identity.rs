use crate::error::{CryptoError, Result};
use nc_core::DeviceId;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use std::path::Path;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

pub struct Identity {
    secret: StaticSecret,
    public: [u8; 32],
    device_id: DeviceId,
}

impl Identity {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        Self::from_secret(secret)
    }

    pub fn from_seed(mut seed: [u8; 32]) -> Self {
        let secret = StaticSecret::from(seed);
        seed.zeroize();
        Self::from_secret(secret)
    }

    fn from_secret(secret: StaticSecret) -> Self {
        let public = PublicKey::from(&secret).to_bytes();
        let digest = Sha256::digest(public);
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest[..16]);
        Self {
            secret,
            public,
            device_id: DeviceId::from_bytes(id),
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public
    }

    pub fn device_id(&self) -> DeviceId {
        self.device_id
    }

    pub fn export_seed(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let bytes = std::fs::read(path)?;
            if bytes.len() != 32 {
                return Err(CryptoError::InvalidKey(format!(
                    "身份文件长度应为 32，实际 {}",
                    bytes.len()
                )));
            }
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            Ok(Self::from_seed(seed))
        } else {
            let identity = Self::generate();
            identity.save(path)?;
            Ok(identity)
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut seed = self.export_seed();
        let result = std::fs::write(path, seed);
        seed.zeroize();
        result?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub(crate) fn secret(&self) -> &StaticSecret {
        &self.secret
    }
}

