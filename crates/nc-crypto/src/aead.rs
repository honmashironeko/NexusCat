use crate::error::{CryptoError, Result};
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use rand::RngCore;
use std::collections::HashSet;
use std::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct AeadKey([u8; 32]);

impl AeadKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn random_nonce() -> [u8; 12] {
        let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        nonce
    }

    fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new((&self.0).into())
    }

    pub fn seal(&self, nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        self.cipher()
            .encrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::Aead)
    }

    pub fn open(&self, nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        self.cipher()
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::Aead)
    }
}

impl fmt::Debug for AeadKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AeadKey(<redacted>)")
    }
}

#[derive(Debug)]
pub struct ReplayWindow {
    capacity: u64,
    max_seq: u64,
    initialized: bool,
    seen: HashSet<u64>,
}

impl ReplayWindow {
    pub fn new(capacity: u64) -> Self {
        Self {
            capacity: capacity.max(1),
            max_seq: 0,
            initialized: false,
            seen: HashSet::new(),
        }
    }

    pub fn check_and_update(&mut self, seq: u64) -> bool {
        if !self.initialized {
            self.initialized = true;
            self.max_seq = seq;
            self.seen.insert(seq);
            return true;
        }
        if seq > self.max_seq {
            self.max_seq = seq;
        } else if self.max_seq.saturating_sub(seq) >= self.capacity {
            return false;
        }
        if !self.seen.insert(seq) {
            return false;
        }
        if self.seen.len() as u64 > self.capacity.saturating_mul(2) {
            let cutoff = self.max_seq.saturating_sub(self.capacity);
            self.seen.retain(|value| *value >= cutoff);
        }
        true
    }

    pub fn max_seq(&self) -> u64 {
        self.max_seq
    }
}

