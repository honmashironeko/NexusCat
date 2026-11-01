use crate::aead::AeadKey;
use crate::error::{CryptoError, Result};
use crate::identity::Identity;
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

const CONTROL_INFO: &[u8] = b"nexuscat-control-v1";
const SAS_INFO: &[u8] = b"nexuscat-sas-v1";
const MEDIA_INFO: &[u8] = b"nexuscat-media-v1";

pub struct EphemeralKeyPair {
    secret: StaticSecret,
    public: [u8; 32],
}

impl EphemeralKeyPair {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let public = PublicKey::from(&secret).to_bytes();
        Self { secret, public }
    }

    pub fn public_bytes(&self) -> [u8; 32] {
        self.public
    }
}

impl std::fmt::Debug for EphemeralKeyPair {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EphemeralKeyPair(<redacted>)")
    }
}

#[derive(Debug)]
pub struct SessionKeys {
    pub send_key: AeadKey,
    pub recv_key: AeadKey,
    pub media_key: AeadKey,
    pub sas: String,
    pub transcript_fingerprint: [u8; 32],
}

pub fn derive_session_keys(
    initiator: bool,
    identity: &Identity,
    peer_static_public: [u8; 32],
    ephemeral: &EphemeralKeyPair,
    peer_ephemeral_public: [u8; 32],
    transcript: &[u8],
) -> Result<SessionKeys> {
    let peer_ephemeral = PublicKey::from(peer_ephemeral_public);
    let peer_static = PublicKey::from(peer_static_public);

    let mut dh_values = Vec::with_capacity(4);
    for secret in [&ephemeral.secret, identity.secret()] {
        for public in [&peer_ephemeral, &peer_static] {
            let value = secret.diffie_hellman(public);
            if value.as_bytes().iter().all(|byte| *byte == 0) {
                return Err(CryptoError::InvalidKey("X25519 结果为全零".into()));
            }
            dh_values.push(*value.as_bytes());
        }
    }
    dh_values.sort_unstable();
    let mut shared = Vec::with_capacity(128);
    for value in &dh_values {
        shared.extend_from_slice(value);
    }

    let salt = Sha256::digest(transcript);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), &shared);
    let mut okm = [0u8; 64];
    hkdf.expand(CONTROL_INFO, &mut okm)
        .map_err(|_| CryptoError::InvalidKey("HKDF 输出长度非法".into()))?;

    let mut media_bytes = [0u8; 32];
    hkdf.expand(MEDIA_INFO, &mut media_bytes)
        .map_err(|_| CryptoError::InvalidKey("HKDF 媒体密钥输出长度非法".into()))?;

    let mut sas_bytes = [0u8; 8];
    hkdf.expand(SAS_INFO, &mut sas_bytes)
        .map_err(|_| CryptoError::InvalidKey("HKDF SAS 输出长度非法".into()))?;
    let sas_value = u64::from_be_bytes(sas_bytes) % 1_000_000;
    let sas = format!("{sas_value:06}");

    let mut initiator_to_responder = [0u8; 32];
    initiator_to_responder.copy_from_slice(&okm[..32]);
    let mut responder_to_initiator = [0u8; 32];
    responder_to_initiator.copy_from_slice(&okm[32..]);

    let (send, recv) = if initiator {
        (initiator_to_responder, responder_to_initiator)
    } else {
        (responder_to_initiator, initiator_to_responder)
    };

    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&salt);

    shared.zeroize();
    okm.zeroize();
    sas_bytes.zeroize();

    let media_key = AeadKey::from_bytes(media_bytes);
    media_bytes.zeroize();

    Ok(SessionKeys {
        send_key: AeadKey::from_bytes(send),
        recv_key: AeadKey::from_bytes(recv),
        media_key,
        sas,
        transcript_fingerprint: fingerprint,
    })
}

