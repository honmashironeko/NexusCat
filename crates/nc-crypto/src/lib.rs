pub mod aead;
pub mod error;
pub mod handshake;
pub mod identity;
pub mod tls;
pub mod trust;

pub use aead::{AeadKey, ReplayWindow};
pub use error::{CryptoError, Result};
pub use handshake::{derive_session_keys, EphemeralKeyPair, SessionKeys};
pub use identity::Identity;
pub use tls::{
    certificate_fingerprint, client_tls_config, client_tls_config_pinned, generate_certificate,
    load_certificate, server_tls_config, DeviceCertificate,
};
pub use trust::{TrustStore, TrustedDevice};
