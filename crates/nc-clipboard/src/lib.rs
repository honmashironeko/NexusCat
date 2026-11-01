use std::borrow::Cow;
use std::time::Duration;

use arboard::Clipboard;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageFormat};
use nc_core::NexusError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TEXT_HASH_TAG: &[u8] = b"nc-clipboard:text:v1";
const IMAGE_HASH_TAG: &[u8] = b"nc-clipboard:image-png:v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardPayload {
    Text(String),
    ImagePng(Vec<u8>),
}

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("系统剪贴板不可用：{0}")]
    Unavailable(String),
    #[error("系统剪贴板操作失败：{0}")]
    Platform(String),
    #[error("PNG 编解码失败：{0}")]
    Image(String),
}

impl From<ClipboardError> for NexusError {
    fn from(error: ClipboardError) -> Self {
        match error {
            ClipboardError::Unavailable(message) | ClipboardError::Platform(message) => {
                NexusError::Platform(message)
            }
            ClipboardError::Image(message) => NexusError::Media(message),
        }
    }
}

pub fn hash_payload(payload: &ClipboardPayload) -> [u8; 32] {
    let (tag, bytes): (&[u8], &[u8]) = match payload {
        ClipboardPayload::Text(text) => (TEXT_HASH_TAG, text.as_bytes()),
        ClipboardPayload::ImagePng(bytes) => (IMAGE_HASH_TAG, bytes),
    };
    let mut hasher = Sha256::new();
    hasher.update(tag);
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}

pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ClipboardError> {
    if width == 0 || height == 0 {
        return Err(ClipboardError::Image("图像宽高不能为 0".into()));
    }
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| ClipboardError::Image(format!("图像尺寸 {width}x{height} 溢出")))?;
    if rgba.len() != expected_len {
        return Err(ClipboardError::Image(format!(
            "RGBA 数据长度 {} 与尺寸 {width}x{height} 不匹配",
            rgba.len()
        )));
    }

    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|error| ClipboardError::Image(error.to_string()))?;
    Ok(png)
}

pub fn decode_png(png: &[u8]) -> Result<(Vec<u8>, u32, u32), ClipboardError> {
    let image = image::load_from_memory_with_format(png, ImageFormat::Png)
        .map_err(|error| ClipboardError::Image(error.to_string()))?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok((rgba.into_raw(), width, height))
}

#[derive(Debug)]
pub struct ClipboardWatcher {
    poll_interval: Duration,
    last_hash: Option<[u8; 32]>,
}

impl ClipboardWatcher {
    pub fn new(poll_interval: Duration) -> Self {
        Self {
            poll_interval,
            last_hash: None,
        }
    }

    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    pub fn poll(&mut self) -> Result<Option<ClipboardPayload>, ClipboardError> {
        let Some(payload) = read_clipboard()? else {
            return Ok(None);
        };
        let hash = hash_payload(&payload);
        if self.last_hash == Some(hash) {
            return Ok(None);
        }
        self.last_hash = Some(hash);
        Ok(Some(payload))
    }

    pub fn set(&mut self, payload: ClipboardPayload) -> Result<(), ClipboardError> {
        self.write(payload)
    }

    pub fn set_remote(&mut self, payload: ClipboardPayload) -> Result<(), ClipboardError> {
        self.write(payload)
    }

    fn write(&mut self, payload: ClipboardPayload) -> Result<(), ClipboardError> {
        let hash = hash_payload(&payload);
        let mut clipboard = Clipboard::new().map_err(unavailable)?;
        match payload {
            ClipboardPayload::Text(text) => clipboard
                .set_text(text)
                .map_err(|error| ClipboardError::Platform(format!("写入文本失败：{error}")))?,
            ClipboardPayload::ImagePng(png) => {
                let (rgba, width, height) = decode_png(&png)?;
                let image = arboard::ImageData {
                    width: width as usize,
                    height: height as usize,
                    bytes: Cow::Owned(rgba),
                };
                clipboard
                    .set_image(image)
                    .map_err(|error| ClipboardError::Platform(format!("写入图像失败：{error}")))?;
            }
        }
        self.last_hash = Some(hash);
        Ok(())
    }
}

fn read_clipboard() -> Result<Option<ClipboardPayload>, ClipboardError> {
    let mut clipboard = Clipboard::new().map_err(unavailable)?;

    match clipboard.get_text() {
        Ok(text) if !text.is_empty() => return Ok(Some(ClipboardPayload::Text(text))),
        Ok(_) | Err(arboard::Error::ContentNotAvailable | arboard::Error::ConversionFailure) => {}
        Err(error) => {
            return Err(ClipboardError::Platform(format!(
                "读取剪贴板文本失败：{error}"
            )))
        }
    }

    match clipboard.get_image() {
        Ok(image) => {
            let (Ok(width), Ok(height)) = (u32::try_from(image.width), u32::try_from(image.height))
            else {
                return Ok(None);
            };
            if width == 0 || height == 0 || image.bytes.is_empty() {
                return Ok(None);
            }
            match encode_png(image.bytes.as_ref(), width, height) {
                Ok(png) => Ok(Some(ClipboardPayload::ImagePng(png))),
                Err(_) => Ok(None),
            }
        }
        Err(arboard::Error::ContentNotAvailable | arboard::Error::ConversionFailure) => Ok(None),
        Err(error) => Err(ClipboardError::Platform(format!(
            "读取剪贴板图像失败：{error}"
        ))),
    }
}

fn unavailable(error: arboard::Error) -> ClipboardError {
    ClipboardError::Unavailable(error.to_string())
}
