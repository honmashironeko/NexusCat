use std::fmt;

use rand::rngs::OsRng;
use rand::RngCore;
use zeroize::Zeroize;

#[derive(Clone)]
pub struct MediaKey([u8; 32]);

impl MediaKey {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self::new(bytes)
    }

    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for MediaKey {
    fn from(bytes: [u8; 32]) -> Self {
        Self::new(bytes)
    }
}

impl fmt::Debug for MediaKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MediaKey(<redacted>)")
    }
}

impl Drop for MediaKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

