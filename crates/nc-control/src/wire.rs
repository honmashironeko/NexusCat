use nc_core::{DeviceInfo, NexusError, Result};
use nc_crypto::AeadKey;
use nc_proto::{
    decode_control, decode_handshake, encode_control, encode_handshake, ControlFrame,
    ControlMessage, FrameKind, HandshakeMessage, MAX_CONTROL_FRAME_BYTES,
};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const FRAME_HEADER_BYTES: usize = 4 + 1 + 8;
const AEAD_NONCE_BYTES: usize = 12;
const AEAD_TAG_BYTES: usize = 16;
const FRAME_AAD_PREFIX: &[u8] = b"nexuscat-control-frame-v1";
const TRANSCRIPT_LABEL: &[u8] = b"nexuscat-control-handshake-v1";

pub(crate) async fn read_frame<R>(reader: &mut R) -> Result<ControlFrame>
where
    R: AsyncRead + Unpin,
{
    let mut prefix = [0u8; 4];
    reader
        .read_exact(&mut prefix)
        .await
        .map_err(network_error)?;
    let body_len = u32::from_be_bytes(prefix) as usize;
    let total = FRAME_HEADER_BYTES
        .checked_add(body_len)
        .ok_or_else(|| NexusError::Protocol("帧长度溢出".into()))?;
    if total > MAX_CONTROL_FRAME_BYTES {
        return Err(NexusError::Protocol(format!(
            "帧超过上限：{total} > {MAX_CONTROL_FRAME_BYTES} 字节"
        )));
    }

    let mut encoded = vec![0u8; total];
    encoded[..4].copy_from_slice(&prefix);
    reader
        .read_exact(&mut encoded[4..])
        .await
        .map_err(network_error)?;
    nc_proto::decode_frame(&encoded).map_err(|error| NexusError::Protocol(error.to_string()))
}

pub(crate) async fn write_frame<W>(writer: &mut W, frame: &ControlFrame) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let encoded =
        nc_proto::encode_frame(frame).map_err(|error| NexusError::Protocol(error.to_string()))?;
    writer.write_all(&encoded).await.map_err(network_error)
}

pub(crate) fn handshake_frame(message: &HandshakeMessage) -> Result<ControlFrame> {
    let body =
        encode_handshake(message).map_err(|error| NexusError::Protocol(error.to_string()))?;
    Ok(ControlFrame::handshake(body))
}

pub(crate) fn decode_handshake_frame(frame: &ControlFrame) -> Result<HandshakeMessage> {
    if frame.kind != FrameKind::Handshake || frame.seq != 0 {
        return Err(NexusError::Protocol(
            "握手阶段收到非握手帧或非零序号".into(),
        ));
    }
    decode_handshake(&frame.body).map_err(|error| NexusError::Protocol(error.to_string()))
}

pub(crate) fn encrypt_control_frame(
    key: &AeadKey,
    seq: u64,
    message: &ControlMessage,
) -> Result<ControlFrame> {
    if seq == 0 {
        return Err(NexusError::Protocol("控制帧序号必须从 1 开始".into()));
    }
    let plaintext =
        encode_control(message).map_err(|error| NexusError::Protocol(error.to_string()))?;
    let nonce = AeadKey::random_nonce();
    let ciphertext = key
        .seal(&nonce, &frame_aad(seq), &plaintext)
        .map_err(|error| NexusError::Crypto(error.to_string()))?;
    let mut body = Vec::with_capacity(AEAD_NONCE_BYTES + ciphertext.len());
    body.extend_from_slice(&nonce);
    body.extend_from_slice(&ciphertext);
    Ok(ControlFrame::encrypted(seq, body))
}

pub(crate) fn decrypt_control_frame(key: &AeadKey, frame: &ControlFrame) -> Result<ControlMessage> {
    if frame.kind != FrameKind::Encrypted {
        return Err(NexusError::Protocol("收到非加密控制帧".into()));
    }
    if frame.seq == 0 {
        return Err(NexusError::Protocol("控制帧序号为零".into()));
    }
    if frame.body.len() < AEAD_NONCE_BYTES + AEAD_TAG_BYTES {
        return Err(NexusError::Protocol("加密帧载荷过短".into()));
    }
    let mut nonce = [0u8; AEAD_NONCE_BYTES];
    nonce.copy_from_slice(&frame.body[..AEAD_NONCE_BYTES]);
    let ciphertext = &frame.body[AEAD_NONCE_BYTES..];
    let plaintext = key
        .open(&nonce, &frame_aad(frame.seq), ciphertext)
        .map_err(|error| NexusError::Crypto(error.to_string()))?;
    decode_control(&plaintext).map_err(|error| NexusError::Protocol(error.to_string()))
}

fn frame_aad(seq: u64) -> Vec<u8> {
    let mut aad = Vec::with_capacity(FRAME_AAD_PREFIX.len() + 1 + 8);
    aad.extend_from_slice(FRAME_AAD_PREFIX);
    aad.push(1);
    aad.extend_from_slice(&seq.to_be_bytes());
    aad
}

pub(crate) fn build_transcript(hello: &[u8], hello_ack: &[u8]) -> Vec<u8> {
    let mut transcript = Vec::with_capacity(TRANSCRIPT_LABEL.len() + hello.len() + hello_ack.len());
    transcript.extend_from_slice(TRANSCRIPT_LABEL);
    transcript.extend_from_slice(hello);
    transcript.extend_from_slice(hello_ack);
    transcript
}

pub(crate) fn validate_peer_device(device: &DeviceInfo, static_public: &[u8; 32]) -> Result<()> {
    if &device_id_from_public(static_public) != device.id.as_bytes() {
        return Err(NexusError::Protocol(
            "设备 ID 与静态公钥的 SHA-256 前缀不一致".into(),
        ));
    }
    Ok(())
}

fn device_id_from_public(static_public: &[u8; 32]) -> [u8; 16] {
    let digest = Sha256::digest(static_public);
    let mut device_id = [0u8; 16];
    device_id.copy_from_slice(&digest[..16]);
    device_id
}

pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    use rand::RngCore;
    let mut bytes = [0u8; N];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
}

fn network_error(error: std::io::Error) -> NexusError {
    NexusError::Network(error.to_string())
}

