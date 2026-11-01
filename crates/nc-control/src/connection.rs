use crate::config::{ConnectionPurpose, ControlConfig, PairingIntent};
use crate::driver::{self, Command, Role, SessionStart};
use crate::event::ControlEvent;
use crate::wire;
use nc_core::{NexusError, Result};
use nc_crypto::{
    certificate_fingerprint, client_tls_config, client_tls_config_pinned, derive_session_keys,
    server_tls_config, EphemeralKeyPair, TrustedDevice,
};
use nc_proto::{ControlMessage, HandshakeMessage, Hello, HelloAck, HANDSHAKE_FEATURES};
use rustls::pki_types::ServerName;
use std::borrow::Borrow;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_rustls::{TlsAcceptor, TlsConnector};

const TLS_SERVER_NAME: &str = "nexuscat";

pub struct ControlConnection {
    commands: mpsc::UnboundedSender<Command>,
    closed: Arc<AtomicBool>,
}

impl ControlConnection {
    pub async fn connect(
        config: ControlConfig,
        addr: SocketAddr,
        intent: PairingIntent,
        purpose: ConnectionPurpose,
        events: mpsc::Sender<ControlEvent>,
    ) -> Result<Self> {
        config.validate()?;
        let tls_config = match config.pinned_tls_fingerprint(intent)? {
            Some(fingerprint) => client_tls_config_pinned(fingerprint).map_err(crypto_error)?,
            None => client_tls_config().map_err(crypto_error)?,
        };

        let tcp = timeout(config.connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| NexusError::Network(format!("连接 {addr} 超时")))?
            .map_err(network_error)?;
        tcp.set_nodelay(true).map_err(network_error)?;

        let connector = TlsConnector::from(tls_config);
        let server_name = ServerName::try_from(TLS_SERVER_NAME)
            .map_err(|error| NexusError::Crypto(format!("非法 TLS ServerName：{error}")))?;
        let mut tls = timeout(
            config.handshake_timeout,
            connector.connect(server_name, tcp),
        )
        .await
        .map_err(|_| NexusError::Network("TLS 握手超时".into()))?
        .map_err(|error| NexusError::Crypto(format!("TLS 握手失败：{error}")))?;
        let observed_fingerprint = client_peer_fingerprint(&tls)?;

        let ephemeral = EphemeralKeyPair::generate();
        let hello = Hello {
            device: config.local_device_info(),
            static_public: config.identity.public_key(),
            ephemeral_public: ephemeral.public_bytes(),
            random: wire::random_bytes(),
            wants_pairing: intent == PairingIntent::Pair,
            purpose: purpose.link_purpose(),
            features: HANDSHAKE_FEATURES,
        };
        let hello_frame = wire::handshake_frame(&HandshakeMessage::Hello(hello))?;
        let hello_bytes = hello_frame.body.clone();
        timeout(
            config.handshake_timeout,
            wire::write_frame(&mut tls, &hello_frame),
        )
        .await
        .map_err(|_| NexusError::Network("发送 Hello 超时".into()))??;

        let ack_frame = timeout(config.handshake_timeout, wire::read_frame(&mut tls))
            .await
            .map_err(|_| NexusError::Network("等待 HelloAck 超时".into()))??;
        let ack = match wire::decode_handshake_frame(&ack_frame)? {
            HandshakeMessage::HelloAck(ack) => ack,
            HandshakeMessage::PairResult {
                accepted: false,
                reason,
            } => {
                return Err(NexusError::Business(
                    reason.unwrap_or_else(|| "对端拒绝连接".into()),
                ));
            }
            _ => {
                return Err(NexusError::Protocol("连接阶段收到意外的握手消息".into()));
            }
        };
        wire::validate_peer_device(&ack.device, &ack.static_public)?;
        if ack.features != HANDSHAKE_FEATURES {
            return Err(NexusError::Protocol("对端握手结构不兼容".into()));
        }

        let record = config.trusted_record(&ack.device.id)?;
        let mut pending_certificate_update = false;
        match intent {
            PairingIntent::Trusted => {
                let record = record.as_ref().ok_or_else(|| {
                    NexusError::Business("目标设备不在信任存储中，不能以已信任模式连接".into())
                })?;
                if record.static_public != ack.static_public {
                    return Err(NexusError::Crypto("对端静态公钥与信任记录不一致".into()));
                }
                if !ack.trusted || ack.pairing_required {
                    return Err(NexusError::Protocol("对端未按已信任模式接受连接".into()));
                }
                verify_certificate_fingerprint(record, observed_fingerprint)?;
                pending_certificate_update = record.certificate_fingerprint == [0u8; 32];
            }
            PairingIntent::Pair => {
                if !ack.pairing_required && !ack.trusted {
                    return Err(NexusError::Protocol("对端既不信任本机也不提供配对".into()));
                }
                if !ack.pairing_required && ack.trusted {
                    let record = record.as_ref().ok_or_else(|| {
                        NexusError::Business("对端已信任本机，但本地缺少该设备的信任记录".into())
                    })?;
                    if record.static_public != ack.static_public {
                        return Err(NexusError::Crypto("对端静态公钥与信任记录不一致".into()));
                    }
                    verify_certificate_fingerprint(record, observed_fingerprint)?;
                    pending_certificate_update = record.certificate_fingerprint == [0u8; 32];
                }
            }
        }

        let transcript = wire::build_transcript(&hello_bytes, &ack_frame.body);
        let session = derive_session_keys(
            true,
            &config.identity,
            ack.static_public,
            &ephemeral,
            ack.ephemeral_public,
            &transcript,
        )
        .map_err(crypto_error)?;
        let pairing = ack.pairing_required;
        let peer = ack.device;
        let started = driver::spawn(SessionStart {
            stream: tls,
            role: Role::Client,
            config,
            events,
            peer,
            peer_static: ack.static_public,
            peer_certificate_fingerprint: observed_fingerprint,
            session,
            pairing,
            pending_certificate_update,
            purpose: purpose.link_purpose(),
        });
        Ok(Self {
            commands: started.commands,
            closed: started.closed,
        })
    }

    pub async fn accept(
        listener: impl Borrow<TcpListener>,
        config: ControlConfig,
        events: mpsc::Sender<ControlEvent>,
    ) -> Result<Self> {
        config.validate()?;
        let (tcp, _) = timeout(config.connect_timeout, listener.borrow().accept())
            .await
            .map_err(|_| NexusError::Network("等待控制连接超时".into()))?
            .map_err(network_error)?;
        Self::accept_stream(tcp, config, events).await
    }

    pub async fn accept_stream(
        tcp: TcpStream,
        config: ControlConfig,
        events: mpsc::Sender<ControlEvent>,
    ) -> Result<Self> {
        config.validate()?;
        tcp.set_nodelay(true).map_err(network_error)?;

        let acceptor =
            TlsAcceptor::from(server_tls_config(&config.certificate).map_err(crypto_error)?);
        let mut tls = timeout(config.handshake_timeout, acceptor.accept(tcp))
            .await
            .map_err(|_| NexusError::Network("TLS 握手超时".into()))?
            .map_err(|error| NexusError::Crypto(format!("TLS 握手失败：{error}")))?;

        let hello_frame = timeout(config.handshake_timeout, wire::read_frame(&mut tls))
            .await
            .map_err(|_| NexusError::Network("等待 Hello 超时".into()))??;
        let hello = match wire::decode_handshake_frame(&hello_frame)? {
            HandshakeMessage::Hello(hello) => hello,
            _ => {
                return Err(NexusError::Protocol("客户端首条消息不是 Hello".into()));
            }
        };
        if let Err(error) = wire::validate_peer_device(&hello.device, &hello.static_public) {
            let _ = send_handshake_message(
                &mut tls,
                &HandshakeMessage::PairResult {
                    accepted: false,
                    reason: Some(error.to_string()),
                },
                config.handshake_timeout,
            )
            .await;
            return Err(error);
        }
        if hello.features != HANDSHAKE_FEATURES {
            let reason = "客户端握手结构不兼容".to_string();
            let _ = send_handshake_message(
                &mut tls,
                &HandshakeMessage::PairResult {
                    accepted: false,
                    reason: Some(reason.clone()),
                },
                config.handshake_timeout,
            )
            .await;
            return Err(NexusError::Protocol(reason));
        }

        let record = config.trusted_record(&hello.device.id)?;
        if let Some(record) = &record {
            if record.static_public != hello.static_public {
                let reason = "客户端静态公钥与信任记录不一致".to_string();
                let _ = send_handshake_message(
                    &mut tls,
                    &HandshakeMessage::PairResult {
                        accepted: false,
                        reason: Some(reason.clone()),
                    },
                    config.handshake_timeout,
                )
                .await;
                return Err(NexusError::Crypto(reason));
            }
        }
        let trusted = config.is_trusted(&hello.device.id, &hello.static_public)?;
        if !trusted && !hello.wants_pairing {
            let reason = "设备未配对且未请求配对".to_string();
            let _ = send_handshake_message(
                &mut tls,
                &HandshakeMessage::PairResult {
                    accepted: false,
                    reason: Some(reason.clone()),
                },
                config.handshake_timeout,
            )
            .await;
            let _ = events
                .send(ControlEvent::PairingFailed {
                    reason: reason.clone(),
                })
                .await;
            return Err(NexusError::Business(reason));
        }

        let ephemeral = EphemeralKeyPair::generate();
        let ack = HelloAck {
            device: config.local_device_info(),
            static_public: config.identity.public_key(),
            ephemeral_public: ephemeral.public_bytes(),
            random: wire::random_bytes(),
            trusted,
            pairing_required: !trusted,
            features: HANDSHAKE_FEATURES,
        };
        let ack_frame = wire::handshake_frame(&HandshakeMessage::HelloAck(ack.clone()))?;
        let ack_bytes = ack_frame.body.clone();
        timeout(
            config.handshake_timeout,
            wire::write_frame(&mut tls, &ack_frame),
        )
        .await
        .map_err(|_| NexusError::Network("发送 HelloAck 超时".into()))??;

        let transcript = wire::build_transcript(&hello_frame.body, &ack_bytes);
        let session = derive_session_keys(
            false,
            &config.identity,
            hello.static_public,
            &ephemeral,
            hello.ephemeral_public,
            &transcript,
        )
        .map_err(crypto_error)?;
        let started = driver::spawn(SessionStart {
            stream: tls,
            role: Role::Server,
            config,
            events,
            peer: hello.device,
            peer_static: hello.static_public,
            peer_certificate_fingerprint: [0u8; 32],
            session,
            pairing: !trusted,
            pending_certificate_update: false,
            purpose: hello.purpose,
        });
        Ok(Self {
            commands: started.commands,
            closed: started.closed,
        })
    }

    pub fn confirm_sas(&self, accept: bool) -> Result<()> {
        self.ensure_open()?;
        self.commands
            .send(Command::ConfirmSas(accept))
            .map_err(|_| NexusError::NotConnected("控制连接已关闭".into()))
    }

    pub fn send(&self, message: ControlMessage) -> Result<()> {
        self.ensure_open()?;
        self.commands
            .send(Command::Send(message))
            .map_err(|_| NexusError::NotConnected("控制连接已关闭".into()))
    }

    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let _ = self.commands.send(Command::Close);
        }
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(NexusError::NotConnected("控制连接已关闭".into()));
        }
        Ok(())
    }
}

impl Drop for ControlConnection {
    fn drop(&mut self) {
        self.close();
    }
}

fn verify_certificate_fingerprint(
    record: &TrustedDevice,
    observed_fingerprint: [u8; 32],
) -> Result<()> {
    if record.certificate_fingerprint != [0u8; 32]
        && record.certificate_fingerprint != observed_fingerprint
    {
        return Err(NexusError::Crypto("对端证书指纹与信任记录不一致".into()));
    }
    Ok(())
}

fn client_peer_fingerprint(
    stream: &tokio_rustls::client::TlsStream<TcpStream>,
) -> Result<[u8; 32]> {
    let (_, connection) = stream.get_ref();
    let certificate = connection
        .peer_certificates()
        .and_then(|certificates| certificates.first())
        .ok_or_else(|| NexusError::Crypto("TLS 对端未提供证书".into()))?;
    Ok(certificate_fingerprint(certificate.as_ref()))
}

async fn send_handshake_message<W>(
    writer: &mut W,
    message: &HandshakeMessage,
    write_timeout: Duration,
) -> std::result::Result<(), NexusError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let frame = wire::handshake_frame(message)?;
    timeout(write_timeout, wire::write_frame(writer, &frame))
        .await
        .map_err(|_| NexusError::Network("发送握手消息超时".into()))?
}

fn network_error(error: std::io::Error) -> NexusError {
    NexusError::Network(error.to_string())
}

fn crypto_error(error: nc_crypto::CryptoError) -> NexusError {
    NexusError::Crypto(error.to_string())
}
