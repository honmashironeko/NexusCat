use crate::error::{CryptoError, Result};
use crate::identity::Identity;
use rcgen::{CertificateParams, DnType, IsCa};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::sync::{Arc, Once};
use subtle::ConstantTimeEq;

static INSTALL_PROVIDER: Once = Once::new();

fn ensure_provider() {
    INSTALL_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[derive(Debug, Clone)]
pub struct DeviceCertificate {
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint: [u8; 32],
}

pub fn certificate_fingerprint(cert_der: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(cert_der);
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&digest);
    fingerprint
}

pub fn generate_certificate(identity: &Identity) -> Result<DeviceCertificate> {
    let key_pair =
        rcgen::KeyPair::generate().map_err(|error| CryptoError::Certificate(error.to_string()))?;
    let mut params = CertificateParams::new(vec![format!("nexuscat-{}", identity.device_id())])
        .map_err(|error| CryptoError::Certificate(error.to_string()))?;
    params
        .distinguished_name
        .push(DnType::CommonName, "NexusCat Device");
    params.is_ca = IsCa::NoCa;
    let certificate = params
        .self_signed(&key_pair)
        .map_err(|error| CryptoError::Certificate(error.to_string()))?;
    let cert_der = certificate.der().to_vec();
    let fingerprint = certificate_fingerprint(&cert_der);
    Ok(DeviceCertificate {
        cert_pem: certificate.pem(),
        key_pem: key_pair.serialize_pem(),
        fingerprint,
    })
}

pub fn load_certificate(cert_pem: String, key_pem: String) -> Result<DeviceCertificate> {
    let certs = rustls_pemfile::certs(&mut Cursor::new(cert_pem.as_bytes()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| CryptoError::Certificate(error.to_string()))?;
    let first = certs
        .first()
        .ok_or_else(|| CryptoError::Certificate("证书 PEM 为空".into()))?;
    let fingerprint = certificate_fingerprint(first.as_ref());
    Ok(DeviceCertificate {
        cert_pem,
        key_pem,
        fingerprint,
    })
}

pub fn server_tls_config(certificate: &DeviceCertificate) -> Result<Arc<ServerConfig>> {
    ensure_provider();
    let certs = rustls_pemfile::certs(&mut Cursor::new(certificate.cert_pem.as_bytes()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| CryptoError::Tls(error.to_string()))?;
    let key = rustls_pemfile::private_key(&mut Cursor::new(certificate.key_pem.as_bytes()))
        .map_err(|error| CryptoError::Tls(error.to_string()))?
        .ok_or_else(|| CryptoError::Tls("证书缺少私钥".into()))?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|error| CryptoError::Tls(error.to_string()))?;
    Ok(Arc::new(config))
}

pub fn client_tls_config() -> Result<Arc<ClientConfig>> {
    client_tls_config_with_fingerprint(None)
}

pub fn client_tls_config_pinned(fingerprint: [u8; 32]) -> Result<Arc<ClientConfig>> {
    client_tls_config_with_fingerprint(Some(fingerprint))
}

fn client_tls_config_with_fingerprint(
    expected_fingerprint: Option<[u8; 32]>,
) -> Result<Arc<ClientConfig>> {
    ensure_provider();
    let verifier = Arc::new(FingerprintVerifier {
        expected_fingerprint,
    });
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

#[derive(Debug)]
struct FingerprintVerifier {
    expected_fingerprint: Option<[u8; 32]>,
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if let Some(expected) = self.expected_fingerprint {
            let actual = certificate_fingerprint(end_entity.as_ref());
            if !bool::from(actual.ct_eq(&expected)) {
                return Err(rustls::Error::General("服务端证书指纹不匹配".into()));
            }
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        let provider = rustls::crypto::ring::default_provider();
        verify_tls12_signature(
            message,
            cert,
            dss,
            &provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        let provider = rustls::crypto::ring::default_provider();
        verify_tls13_signature(
            message,
            cert,
            dss,
            &provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

