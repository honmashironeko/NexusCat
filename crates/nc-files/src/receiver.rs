use crate::path::{part_path, sanitize_file_name, unique_destination};
use crate::FileProgress;
use nc_core::{NexusError, Result};
use nc_proto::{ControlMessage, FileTransferId};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverState {
    Receiving,
    Finished,
    Aborted,
}

pub struct FileReceiver {
    transfer_id: FileTransferId,
    destination: PathBuf,
    part_path: PathBuf,
    file: Option<File>,
    hasher: Option<Sha256>,
    expected_size: u64,
    expected_sha256: [u8; 32],
    chunk_size: usize,
    bytes_done: u64,
    state: ReceiverState,
}

impl FileReceiver {
    pub fn new(
        transfer_id: FileTransferId,
        file_name: impl AsRef<str>,
        size: u64,
        sha256: [u8; 32],
        destination_dir: impl AsRef<Path>,
        chunk_size: usize,
    ) -> Result<Self> {
        if chunk_size == 0 {
            return Err(NexusError::invalid("分块大小必须大于 0"));
        }
        let safe_name = sanitize_file_name(file_name)?;
        let destination_dir = destination_dir.as_ref();
        fs::create_dir_all(destination_dir)?;
        let destination = unique_destination(destination_dir, &safe_name)?;
        let part_path = part_path(&destination);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)?;

        Ok(Self {
            transfer_id,
            destination,
            part_path,
            file: Some(file),
            hasher: Some(Sha256::new()),
            expected_size: size,
            expected_sha256: sha256,
            chunk_size,
            bytes_done: 0,
            state: ReceiverState::Receiving,
        })
    }

    pub fn accept(&self) -> ControlMessage {
        ControlMessage::FileAccept {
            transfer_id: self.transfer_id,
            offset: 0,
        }
    }

    pub fn write_chunk(&mut self, offset: u64, data: impl AsRef<[u8]>) -> Result<()> {
        if self.state != ReceiverState::Receiving {
            return Err(NexusError::invalid("文件接收器已中止或完成"));
        }

        let data = data.as_ref();
        if offset != self.bytes_done {
            return self.fail(NexusError::invalid(format!(
                "文件块偏移不连续：期望 {}，实际 {offset}",
                self.bytes_done
            )));
        }
        if data.is_empty() {
            return self.fail(NexusError::invalid("文件块不能为空"));
        }
        if data.len() > self.chunk_size {
            return self.fail(NexusError::invalid(format!(
                "文件块超过约定大小：{} > {}",
                data.len(),
                self.chunk_size
            )));
        }
        let end = match offset.checked_add(data.len() as u64) {
            Some(end) => end,
            None => return self.fail(NexusError::invalid("文件块偏移溢出")),
        };
        if end > self.expected_size {
            return self.fail(NexusError::invalid("文件块超出声明的文件大小"));
        }

        let Some(file) = self.file.as_mut() else {
            return self.fail(NexusError::invalid("文件写入句柄不可用"));
        };
        if let Err(error) = file.write_all(data) {
            return self.fail(error.into());
        }
        if let Some(hasher) = self.hasher.as_mut() {
            hasher.update(data);
        }
        self.bytes_done = end;
        Ok(())
    }

    pub fn finish(mut self) -> Result<PathBuf> {
        if self.state != ReceiverState::Receiving {
            return Err(NexusError::invalid("文件接收器已中止或完成"));
        }
        if self.bytes_done != self.expected_size {
            return self.fail(NexusError::invalid(format!(
                "文件大小不完整：期望 {}，实际 {}",
                self.expected_size, self.bytes_done
            )));
        }

        if let Some(mut file) = self.file.take() {
            if let Err(error) = file.flush().and_then(|()| file.sync_all()) {
                return self.fail(error.into());
            }
        } else {
            return self.fail(NexusError::invalid("文件写入句柄不可用"));
        }

        let Some(hasher) = self.hasher.take() else {
            return self.fail(NexusError::invalid("文件哈希状态不可用"));
        };
        let actual: [u8; 32] = hasher.finalize().into();
        if actual != self.expected_sha256 {
            return self.fail(NexusError::invalid("文件 SHA-256 校验失败"));
        }

        if let Err(error) = fs::rename(&self.part_path, &self.destination) {
            return self.fail(error.into());
        }
        self.state = ReceiverState::Finished;
        Ok(self.destination.clone())
    }

    pub fn abort(&mut self) {
        if self.state == ReceiverState::Finished {
            return;
        }
        self.state = ReceiverState::Aborted;
        self.file = None;
        remove_part(&self.part_path);
    }

    pub fn progress(&self) -> FileProgress {
        FileProgress::new(self.bytes_done, self.expected_size)
    }

    fn fail<T>(&mut self, error: NexusError) -> Result<T> {
        self.state = ReceiverState::Aborted;
        self.file = None;
        remove_part(&self.part_path);
        Err(error)
    }
}

impl Drop for FileReceiver {
    fn drop(&mut self) {
        if self.state != ReceiverState::Finished {
            self.file = None;
            remove_part(&self.part_path);
        }
    }
}

fn remove_part(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => warn!(path = %path.display(), %error, "清理文件临时分片失败"),
    }
}
