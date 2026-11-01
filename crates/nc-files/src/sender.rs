use crate::FileProgress;
use nc_core::{NexusError, Result};
use nc_proto::{ControlMessage, FileTransferId};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub struct FileSender {
    transfer_id: FileTransferId,
    file: File,
    file_name: String,
    size: u64,
    sha256: [u8; 32],
    chunk_size: usize,
    bytes_sent: u64,
}

impl FileSender {
    pub fn new(
        transfer_id: FileTransferId,
        path: impl AsRef<Path>,
        chunk_size: usize,
    ) -> Result<Self> {
        let path = path.as_ref();
        validate_chunk_size(chunk_size)?;

        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(NexusError::invalid(format!(
                "发送源不是普通文件：{}",
                path.display()
            )));
        }
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| NexusError::invalid("文件名不是有效 UTF-8"))?
            .to_owned();
        let (size, sha256) = hash_and_rewind(&mut file)?;

        Ok(Self {
            transfer_id,
            file,
            file_name,
            size,
            sha256,
            chunk_size,
            bytes_sent: 0,
        })
    }

    pub fn open(path: impl AsRef<Path>, chunk_size: usize) -> Result<Self> {
        Self::new(FileTransferId::default(), path, chunk_size)
    }

    pub fn offer(&self) -> ControlMessage {
        ControlMessage::FileOffer {
            transfer_id: self.transfer_id,
            file_name: self.file_name.clone(),
            size: self.size,
            sha256: self.sha256,
        }
    }

    pub fn next_chunk(&mut self) -> Result<Option<ControlMessage>> {
        if self.bytes_sent >= self.size {
            return Ok(None);
        }

        let remaining = self.size - self.bytes_sent;
        let requested = remaining.min(self.chunk_size as u64) as usize;
        let mut data = vec![0_u8; requested];
        if let Err(error) = self.file.read_exact(&mut data) {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                return Err(NexusError::invalid("文件在发送过程中被截断"));
            }
            return Err(error.into());
        }

        let offset = self.bytes_sent;
        self.bytes_sent += requested as u64;
        Ok(Some(ControlMessage::FileChunk {
            transfer_id: self.transfer_id,
            offset,
            data,
        }))
    }

    pub fn complete(&self) -> ControlMessage {
        ControlMessage::FileComplete {
            transfer_id: self.transfer_id,
        }
    }

    pub fn progress(&self) -> FileProgress {
        FileProgress::new(self.bytes_sent, self.size)
    }

    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
}

fn validate_chunk_size(chunk_size: usize) -> Result<()> {
    if chunk_size == 0 {
        return Err(NexusError::invalid("分块大小必须大于 0"));
    }
    Ok(())
}

fn hash_and_rewind(file: &mut File) -> Result<(u64, [u8; 32])> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| NexusError::invalid("文件大小超出支持范围"))?;
    }
    file.seek(SeekFrom::Start(0))?;
    Ok((total, hasher.finalize().into()))
}

