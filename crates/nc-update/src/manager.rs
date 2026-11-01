use crate::manifest::{
    hash_file, package_kind_label, parse_version, prepare_manifest, verify_package,
};
use crate::script::{self, PortableScript};
use crate::{PackageKind, UpdateManifest};
use nc_core::{NexusError, Platform, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyOutcome {
    pub version: String,
    pub restart_required: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedUpdate {
    pub manifest: UpdateManifest,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackEntry {
    pub version: String,
    pub kind: PackageKind,
    pub path: PathBuf,
    pub size: u64,
    pub sha256: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct UpdateManager {
    data_dir: PathBuf,
    updates_dir: PathBuf,
    staging_dir: PathBuf,
    versions_dir: PathBuf,
    rollback_path: PathBuf,
    applied_path: PathBuf,
    staged_manifest_path: PathBuf,
    current_version: Version,
}

impl UpdateManager {
    pub fn new(data_dir: impl Into<PathBuf>, current_version: impl AsRef<str>) -> Result<Self> {
        let current_version = parse_version(current_version.as_ref())?;
        let data_dir = std::path::absolute(data_dir.into())?;
        let updates_dir = data_dir.join("updates");
        let staging_dir = updates_dir.join("staging");
        let versions_dir = updates_dir.join("versions");
        let rollback_path = updates_dir.join("rollback.json");
        let applied_path = updates_dir.join("applied.json");
        let staged_manifest_path = updates_dir.join("staged.json");

        fs::create_dir_all(&staging_dir)?;
        fs::create_dir_all(&versions_dir)?;
        if !rollback_path.exists() {
            fs::write(&rollback_path, b"[]")?;
        }

        Ok(Self {
            data_dir,
            updates_dir,
            staging_dir,
            versions_dir,
            rollback_path,
            applied_path,
            staged_manifest_path,
            current_version,
        })
    }

    pub fn prepare_local_package(
        &self,
        path: impl AsRef<Path>,
        version: impl AsRef<str>,
        kind: PackageKind,
    ) -> Result<UpdateManifest> {
        prepare_manifest(path.as_ref(), version.as_ref(), kind)
    }

    pub fn save_staged(&self, staged: &StagedUpdate) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(staged)
            .map_err(|error| NexusError::Update(format!("序列化暂存更新失败：{error}")))?;
        fs::write(&self.staged_manifest_path, bytes)
            .map_err(|error| NexusError::Update(format!("写入暂存更新失败：{error}")))
    }

    pub fn load_staged(&self) -> Option<StagedUpdate> {
        let bytes = fs::read(&self.staged_manifest_path).ok()?;
        let staged: StagedUpdate = serde_json::from_slice(&bytes).ok()?;
        if verify_package(&staged.path, &staged.manifest).is_err() {
            let _ = fs::remove_file(&self.staged_manifest_path);
            return None;
        }
        Some(staged)
    }

    pub fn clear_staged(&self) {
        let _ = fs::remove_file(&self.staged_manifest_path);
    }

    pub fn apply(&self, path: impl AsRef<Path>, manifest: &UpdateManifest) -> Result<ApplyOutcome> {
        self.apply_impl(path.as_ref(), manifest, None, true)
    }

    pub fn apply_with_installer_command(
        &self,
        path: impl AsRef<Path>,
        manifest: &UpdateManifest,
        installer_command: Command,
    ) -> Result<ApplyOutcome> {
        if manifest.kind != PackageKind::Installer {
            return Err(NexusError::invalid(
                "自定义安装命令只能用于 Installer 更新包",
            ));
        }
        self.apply_impl(path.as_ref(), manifest, Some(installer_command), true)
    }

    pub fn list_rollback(&self) -> Vec<RollbackEntry> {
        self.load_rollback_entries()
    }

    pub fn rollback(&self, version: impl AsRef<str>) -> Result<()> {
        self.rollback_impl(version.as_ref(), None, true)
    }

    pub fn rollback_with_installer_command(
        &self,
        version: impl AsRef<str>,
        installer_command: Command,
    ) -> Result<()> {
        self.rollback_impl(version.as_ref(), Some(installer_command), true)
    }

    pub fn read_applied_status(&self) -> Option<String> {
        fs::read_to_string(&self.applied_path).ok()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn updates_dir(&self) -> &Path {
        &self.updates_dir
    }

    pub(crate) fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    fn apply_impl(
        &self,
        path: &Path,
        manifest: &UpdateManifest,
        installer_command: Option<Command>,
        launch: bool,
    ) -> Result<ApplyOutcome> {
        let version = self.validate_apply_manifest(manifest)?;
        verify_package(path, manifest)?;

        match manifest.kind {
            PackageKind::Portable => {
                if installer_command.is_some() {
                    return Err(NexusError::invalid("便携更新不能使用安装器命令"));
                }
                self.apply_portable(path, &version, launch)
            }
            PackageKind::Installer => {
                self.apply_installer(path, &version, installer_command, launch)
            }
        }
    }

    fn validate_apply_manifest(&self, manifest: &UpdateManifest) -> Result<Version> {
        let version = parse_version(&manifest.version)?;
        if manifest.platform != Platform::current() {
            return Err(NexusError::Update(format!(
                "更新包平台不匹配：清单为 {}，当前为 {}",
                manifest.platform,
                Platform::current()
            )));
        }
        Ok(version)
    }

    fn apply_portable(
        &self,
        package_path: &Path,
        version: &Version,
        launch: bool,
    ) -> Result<ApplyOutcome> {
        let install_path = current_executable()?;
        reject_macos_app_bundle_portable(&install_path)?;
        let backup_path = self.versions_dir.join(format!(
            "{}-backup{}",
            self.current_version,
            backup_extension()
        ));
        fs::copy(&install_path, &backup_path)?;
        let (backup_size, backup_sha256) = hash_file(&backup_path)?;

        let extension = package_extension(package_path, executable_extension());
        let staged_executable = self
            .versions_dir
            .join(format!("{version}-portable.{extension}"));
        fs::copy(package_path, &staged_executable)?;

        let version_text = version.to_string();
        let script_path = script::script_path(&self.versions_dir, &version_text, "apply");
        let spec = PortableScript {
            wait_pid: process::id(),
            source: &staged_executable,
            destination: &install_path,
            applied_path: &self.applied_path,
            version: &version_text,
            kind: package_kind_label(PackageKind::Portable),
            status: "applied",
        };
        script::write_portable_script(&script_path, &spec)?;
        let env = script::portable_script_env(&spec);

        self.upsert_rollback(RollbackEntry {
            version: self.current_version.to_string(),
            kind: PackageKind::Portable,
            path: backup_path,
            size: backup_size,
            sha256: backup_sha256,
        })?;

        if launch {
            script::spawn_script_with_env(&script_path, &env)?;
        }

        Ok(ApplyOutcome {
            version: version_text,
            restart_required: true,
            message: format!(
                "已准备便携更新，退出应用后将替换并重启；脚本：{}",
                script_path.display()
            ),
        })
    }

    fn apply_installer(
        &self,
        package_path: &Path,
        version: &Version,
        installer_command: Option<Command>,
        launch: bool,
    ) -> Result<ApplyOutcome> {
        let extension = package_extension(package_path, installer_extension());
        let retained_path = self
            .versions_dir
            .join(format!("{version}-installer.{extension}"));
        fs::copy(package_path, &retained_path)?;

        if let Some(previous) = self.retained_installer(&self.current_version.to_string()) {
            let (size, sha256) = hash_file(&previous)?;
            self.upsert_rollback(RollbackEntry {
                version: self.current_version.to_string(),
                kind: PackageKind::Installer,
                path: previous,
                size,
                sha256,
            })?;
        }

        if launch {
            let installer_pid = match installer_command {
                Some(command) => script::spawn_detached(command)?.id(),
                None => script::spawn_installer(&retained_path)?,
            };
            let version_text = version.to_string();
            let restart_path = script::script_path(&self.versions_dir, &version_text, "restart");
            let restart = current_executable().and_then(|launch_path| {
                script::write_restart_script(
                    &restart_path,
                    process::id(),
                    installer_pid,
                    &launch_path,
                )?;
                let env =
                    script::restart_script_env(process::id(), installer_pid, &launch_path);
                script::spawn_script_with_env(&restart_path, &env)
            });
            if let Err(error) = restart {
                return Err(NexusError::Update(format!(
                    "安装程序已启动，但自动重启脚本创建失败，请手动重启应用：{error}"
                )));
            }
        }

        Ok(ApplyOutcome {
            version: version.to_string(),
            restart_required: true,
            message: format!("已启动版本 {version} 的安装程序，安装完成后将自动重启"),
        })
    }

    fn rollback_impl(
        &self,
        version: &str,
        installer_command: Option<Command>,
        launch: bool,
    ) -> Result<()> {
        let target = parse_version(version)?;
        let target_text = target.to_string();
        let mut entries = self.load_rollback_entries();
        let index = entries
            .iter()
            .position(|entry| entry.version == target_text)
            .ok_or_else(|| NexusError::Update(format!("没有版本 {target_text} 的可回退包")))?;
        let entry = entries[index].clone();
        let managed_path = self.verify_rollback_entry(&entry)?;

        match entry.kind {
            PackageKind::Portable => {
                if installer_command.is_some() {
                    return Err(NexusError::invalid("便携回退不能使用安装器命令"));
                }
                let install_path = current_executable()?;
                reject_macos_app_bundle_portable(&install_path)?;
                let script_path = script::script_path(&self.versions_dir, &target_text, "rollback");
                let spec = PortableScript {
                    wait_pid: process::id(),
                    source: &managed_path,
                    destination: &install_path,
                    applied_path: &self.applied_path,
                    version: &target_text,
                    kind: package_kind_label(PackageKind::Portable),
                    status: "rolled_back",
                };
                script::write_portable_script(&script_path, &spec)?;
                let env = script::portable_script_env(&spec);
                if launch {
                    script::spawn_script_with_env(&script_path, &env)?;
                }
            }
            PackageKind::Installer => {
                if launch {
                    if let Some(command) = installer_command {
                        script::spawn_detached(command)?;
                    } else {
                        let installer_pid = script::spawn_installer(&managed_path)?;
                        let restart_path = script::script_path(
                            &self.versions_dir,
                            &target_text,
                            "rollback-restart",
                        );
                        let restart = current_executable().and_then(|launch_path| {
                            script::write_restart_script(
                                &restart_path,
                                process::id(),
                                installer_pid,
                                &launch_path,
                            )?;
                            let env = script::restart_script_env(
                                process::id(),
                                installer_pid,
                                &launch_path,
                            );
                            script::spawn_script_with_env(&restart_path, &env)
                        });
                        if let Err(error) = restart {
                            return Err(NexusError::Update(format!(
                                "回退安装程序已启动，但自动重启脚本创建失败，请手动重启应用：{error}"
                            )));
                        }
                    }
                }
            }
        }

        entries.remove(index);
        self.save_rollback_entries(&entries)?;
        Ok(())
    }

    fn retained_installer(&self, version: &str) -> Option<PathBuf> {
        let candidates = ["exe", "msi", "pkg"];
        candidates.into_iter().find_map(|extension| {
            let path = self
                .versions_dir
                .join(format!("{version}-installer.{extension}"));
            path.is_file().then_some(path)
        })
    }

    fn verify_rollback_entry(&self, entry: &RollbackEntry) -> Result<PathBuf> {
        let canonical_versions = fs::canonicalize(&self.versions_dir)
            .map_err(|error| NexusError::Update(format!("版本目录不可用：{error}")))?;
        let canonical_path = fs::canonicalize(&entry.path)
            .map_err(|error| NexusError::Update(format!("回退包不存在：{error}")))?;
        if !canonical_path.starts_with(&canonical_versions) {
            return Err(NexusError::Update(
                "回退包路径不在受管理的版本目录内".into(),
            ));
        }
        let metadata = fs::metadata(&canonical_path)
            .map_err(|error| NexusError::Update(format!("无法读取回退包：{error}")))?;
        if !metadata.is_file() || metadata.len() != entry.size {
            return Err(NexusError::Update("回退包大小或类型与记录不一致".into()));
        }
        let (size, sha256) = hash_file(&canonical_path)?;
        if size != entry.size || sha256 != entry.sha256 {
            return Err(NexusError::Update("回退包 SHA-256 与记录不一致".into()));
        }
        Ok(canonical_path)
    }

    fn load_rollback_entries(&self) -> Vec<RollbackEntry> {
        let bytes = match fs::read(&self.rollback_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => {
                warn!(path = %self.rollback_path.display(), %error, "读取回退记录失败");
                return Vec::new();
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(entries) => entries,
            Err(error) => {
                warn!(path = %self.rollback_path.display(), %error, "解析回退记录失败");
                Vec::new()
            }
        }
    }

    fn upsert_rollback(&self, entry: RollbackEntry) -> Result<()> {
        let mut entries = self.load_rollback_entries();
        match entries
            .iter_mut()
            .find(|existing| existing.version == entry.version && existing.kind == entry.kind)
        {
            Some(existing) => *existing = entry,
            None => entries.push(entry),
        }
        self.save_rollback_entries(&entries)
    }

    fn save_rollback_entries(&self, entries: &[RollbackEntry]) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(entries)
            .map_err(|error| NexusError::Update(format!("序列化回退记录失败：{error}")))?;
        fs::write(&self.rollback_path, bytes)?;
        Ok(())
    }
}

fn current_executable() -> Result<PathBuf> {
    std::env::current_exe()
        .map_err(|error| NexusError::Update(format!("无法定位当前可执行文件：{error}")))
}

fn reject_macos_app_bundle_portable(executable: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    if executable.ancestors().any(|ancestor| {
        ancestor
            .extension()
            .is_some_and(|extension| extension.to_str() == Some("app"))
    }) {
        return Err(NexusError::Update(
            "macOS 便携更新会破坏 .app 签名，请使用 .pkg 安装更新".to_string(),
        ));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = executable;
    Ok(())
}

#[cfg(windows)]
const fn executable_extension() -> &'static str {
    "exe"
}

#[cfg(not(windows))]
const fn executable_extension() -> &'static str {
    "bin"
}

#[cfg(windows)]
const fn installer_extension() -> &'static str {
    "exe"
}

#[cfg(not(windows))]
const fn installer_extension() -> &'static str {
    "pkg"
}

#[cfg(windows)]
const fn backup_extension() -> &'static str {
    ".exe"
}

#[cfg(not(windows))]
const fn backup_extension() -> &'static str {
    ""
}

fn package_extension(path: &Path, default: &str) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 16
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| default.to_owned())
}

