use nc_core::{NexusError, Platform, Result};
use nc_proto::{PackageKind, UpdateManifest};
use semver::Version;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

const HASH_BUFFER_BYTES: usize = 64 * 1024;

pub(crate) fn parse_version(text: &str) -> Result<Version> {
    Version::parse(text)
        .map_err(|error| NexusError::invalid(format!("更新版本不是合法 semver：{error}")))
}

pub(crate) const fn package_kind_label(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Portable => "portable",
        PackageKind::Installer => "installer",
    }
}

pub(crate) fn prepare_manifest(
    path: &Path,
    version: &str,
    kind: PackageKind,
) -> Result<UpdateManifest> {
    let parsed = parse_version(version)?;
    let (size, sha256) = hash_file(path)?;
    if size == 0 {
        return Err(NexusError::invalid("更新包不能为空"));
    }
    Ok(UpdateManifest {
        version: parsed.to_string(),
        platform: Platform::current(),
        kind,
        size,
        sha256,
        notes: String::new(),
        force: false,
    })
}

pub(crate) fn verify_package(path: &Path, manifest: &UpdateManifest) -> Result<()> {
    parse_version(&manifest.version)?;
    if manifest.platform != Platform::current() {
        return Err(NexusError::Update(format!(
            "更新包平台不匹配：清单为 {}，当前为 {}",
            manifest.platform,
            Platform::current()
        )));
    }
    if manifest.size == 0 {
        return Err(NexusError::Update("更新清单声明的包大小为 0".into()));
    }
    let metadata = std::fs::metadata(path)
        .map_err(|error| NexusError::Update(format!("无法读取更新包：{error}")))?;
    if !metadata.is_file() {
        return Err(NexusError::invalid(format!(
            "更新包不是普通文件：{}",
            path.display()
        )));
    }
    if metadata.len() != manifest.size {
        return Err(NexusError::Update(format!(
            "更新包大小不匹配：期望 {}，实际 {}",
            manifest.size,
            metadata.len()
        )));
    }
    let (size, sha256) = hash_file(path)?;
    if size != manifest.size || sha256 != manifest.sha256 {
        return Err(NexusError::Update("更新包 SHA-256 校验失败".into()));
    }
    Ok(())
}

pub(crate) fn hash_file(path: &Path) -> Result<(u64, [u8; 32])> {
    let mut file = File::open(path)?;
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
            .ok_or_else(|| NexusError::invalid("更新包大小超出支持范围"))?;
    }
    Ok((total, hasher.finalize().into()))
}
