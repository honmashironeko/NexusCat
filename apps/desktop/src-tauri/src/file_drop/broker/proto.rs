use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Pipes::PeekNamedPipe;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_DROP_PATHS: usize = 1024;
pub const MAX_DROP_PATH_BYTES: usize = 512 * 1024;
pub const POLL_INTERVAL: Duration = Duration::from_millis(15);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub token: String,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Show {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Hide,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Enter { count: u32, x: i32, y: i32 },
    Over { x: i32, y: i32 },
    Leave,
    Drop { paths: Vec<String>, x: i32, y: i32 },
    Error { message: String },
}

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "文件拖放代理消息超过大小上限",
        ));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<T> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "文件拖放代理消息长度非法",
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn read_available(pipe: &File) -> io::Result<Option<Vec<u8>>> {
    let mut available = 0_u32;
    unsafe {
        PeekNamedPipe(
            HANDLE(pipe.as_raw_handle()),
            None,
            0,
            None,
            Some(&mut available),
            None,
        )
    }
    .map_err(io::Error::other)?;
    if available == 0 {
        return Ok(None);
    }
    let mut buffer = vec![0_u8; available as usize];
    let mut read = 0_usize;
    let mut reader = pipe;
    while read < buffer.len() {
        match reader.read(&mut buffer[read..]) {
            Ok(0) => break,
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    buffer.truncate(read);
    Ok(Some(buffer))
}

#[derive(Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
}

impl FrameReader {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    pub fn next<T: DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
        if self.buffer.len() < 4 {
            return Ok(None);
        }
        let length = u32::from_le_bytes(self.buffer[..4].try_into().expect("长度头")) as usize;
        if length == 0 || length > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "文件拖放代理消息长度非法",
            ));
        }
        if self.buffer.len() < 4 + length {
            return Ok(None);
        }
        let value = serde_json::from_slice(&self.buffer[4..4 + length])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.buffer.drain(..4 + length);
        Ok(Some(value))
    }
}

