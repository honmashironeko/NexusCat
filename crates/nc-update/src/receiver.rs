use crate::manifest::{package_kind_label, parse_version};
use crate::{UpdateManager, UpdateManifest};
use nc_core::{NexusError, Platform, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Borrow;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UpdateProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub percent: f64,
}

impl UpdateProgress {
    fn new(bytes_done: u64, bytes_total: u64) -> Self {
        let percent = if bytes_total == 0 {
            100.0
        } else {
            bytes_done as f64 * 100.0 / bytes_total as f64
        };
        Self {
            bytes_done,
            bytes_total,
            percent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverState {
    Receiving,
    Finished,
    Aborted,
}

#[derive(Debug)]
pub struct UpdateReceiver {
    final_path: PathBuf,
    part_path: PathBuf,
    file: Option<File>,
    hasher: Option<Sha256>,
    expected_size: u64,
    expected_sha256: [u8; 32],
    version: String,
    bytes_done: u64,
    state: ReceiverState,
}

impl UpdateReceiver {
    pub fn new(manager: impl Borrow<UpdateManager>, manifest: UpdateManifest) -> Result<Self> {
        let manager = manager.borrow();
        let parsed_version = parse_version(&manifest.version)?;
        if manifest.platform != Platform::current() {
            return Err(NexusError::Update(format!(
                "更新包平台不匹配：清单为 {}，当前为 {}",
                manifest.platform,
                Platform::current()
            )));
        }
        if manifest.size == 0 {
            return Err(NexusError::Update("更新包大小不能为 0".into()));
        }

        let version = parsed_version.to_string();
        let file_name = format!("{version}-{}.pkg", package_kind_label(manifest.kind));
        let final_path = manager.staging_dir().join(file_name);
        let part_path = append_part_suffix(&final_path);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&part_path)?;

        Ok(Self {
            final_path,
            part_path,
            file: Some(file),
            hasher: Some(Sha256::new()),
            expected_size: manifest.size,
            expected_sha256: manifest.sha256,
            version,
            bytes_done: 0,
            state: ReceiverState::Receiving,
        })
    }

    pub fn write_chunk(&mut self, offset: u64, data: impl AsRef<[u8]>) -> Result<()> {
        if self.state != ReceiverState::Receiving {
            return Err(NexusError::Update("更新接收器已结束".into()));
        }
        let data = data.as_ref();
        if offset != self.bytes_done {
            return self.fail(NexusError::Update(format!(
                "更新块偏移不连续：期望 {}，实际 {offset}",
                self.bytes_done
            )));
        }
        if data.is_empty() {
            return self.fail(NexusError::Update("更新块不能为空".into()));
        }
        let end = match offset.checked_add(data.len() as u64) {
            Some(end) => end,
            None => return self.fail(NexusError::Update("更新块偏移溢出".into())),
        };
        if end > self.expected_size {
            return self.fail(NexusError::Update("更新块超出清单声明的大小".into()));
        }

        let Some(file) = self.file.as_mut() else {
            return self.fail(NexusError::Update("更新包写入句柄不可用".into()));
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
            return Err(NexusError::Update("更新接收器已结束".into()));
        }
        if self.bytes_done != self.expected_size {
            return self.fail(NexusError::Update(format!(
                "更新包大小不完整：期望 {}，实际 {}",
                self.expected_size, self.bytes_done
            )));
        }

        if let Some(mut file) = self.file.take() {
            if let Err(error) = file.flush().and_then(|()| file.sync_all()) {
                return self.fail(error.into());
            }
        } else {
            return self.fail(NexusError::Update("更新包写入句柄不可用".into()));
        }

        let Some(hasher) = self.hasher.take() else {
            return self.fail(NexusError::Update("更新包哈希状态不可用".into()));
        };
        let actual: [u8; 32] = hasher.finalize().into();
        if actual != self.expected_sha256 {
            return self.fail(NexusError::Update(format!(
                "更新包 {} 的 SHA-256 校验失败",
                self.version
            )));
        }

        if self.final_path.exists() {
            if let Err(error) = fs::remove_file(&self.final_path) {
                return self.fail(error.into());
            }
        }
        if let Err(error) = fs::rename(&self.part_path, &self.final_path) {
            return self.fail(error.into());
        }
        self.state = ReceiverState::Finished;
        Ok(self.final_path.clone())
    }

    pub fn progress(&self) -> UpdateProgress {
        UpdateProgress::new(self.bytes_done, self.expected_size)
    }

    fn fail<T>(&mut self, error: NexusError) -> Result<T> {
        self.state = ReceiverState::Aborted;
        self.file = None;
        remove_part(&self.part_path);
        Err(error)
    }
}

impl Drop for UpdateReceiver {
    fn drop(&mut self) {
        if self.state != ReceiverState::Finished {
            self.file = None;
            remove_part(&self.part_path);
        }
    }
}

fn append_part_suffix(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(".part");
    PathBuf::from(value)
}

fn remove_part(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => warn!(path = %path.display(), %error, "清理更新包临时分片失败"),
    }
}
