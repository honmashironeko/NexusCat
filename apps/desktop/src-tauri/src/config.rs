use nc_core::AppSettings;
use nc_crypto::{generate_certificate, load_certificate, DeviceCertificate, Identity, TrustStore};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub local_dir: PathBuf,
    pub settings_file: PathBuf,
    pub identity_file: PathBuf,
    pub certificate_file: PathBuf,
    pub certificate_key_file: PathBuf,
    pub trust_file: PathBuf,
    pub layout_file: PathBuf,
    pub logs_dir: PathBuf,
    pub updates_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub test_mode: bool,
}

impl AppPaths {
    pub fn resolve(test_mode: bool) -> std::io::Result<Self> {
        if test_mode {
            let base = std::env::var_os("NEXUSCAT_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::temp_dir().join(format!("nexuscat-test-{}", std::process::id()))
                });
            return Ok(Self::from_base(base.clone(), base, true));
        }

        let project = directories::ProjectDirs::from("app", "NexusCat", "NexusCat")
            .ok_or_else(|| std::io::Error::other("无法解析用户数据目录"))?;
        let data_dir = project.data_dir().to_path_buf();
        let local_dir = project.data_local_dir().to_path_buf();
        let downloads = directories::UserDirs::new()
            .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| data_dir.join("Downloads"))
            .join("NexusCat");
        Ok(Self::from_base(data_dir, local_dir, false).with_downloads(downloads))
    }

    pub fn from_base(data_dir: PathBuf, local_dir: PathBuf, test_mode: bool) -> Self {
        let logs_dir = local_dir.join("logs");
        let updates_dir = local_dir.join("updates");
        let cache_dir = local_dir.join("cache");
        let downloads_dir = data_dir.join("downloads");
        Self {
            settings_file: data_dir.join("settings.json"),
            identity_file: data_dir.join("identity.bin"),
            certificate_file: data_dir.join("certificate.pem"),
            certificate_key_file: data_dir.join("certificate-key.pem"),
            trust_file: data_dir.join("trust.json"),
            layout_file: data_dir.join("layout.json"),
            logs_dir,
            updates_dir,
            downloads_dir,
            cache_dir,
            data_dir,
            local_dir,
            test_mode,
        }
    }

    fn with_downloads(mut self, downloads: PathBuf) -> Self {
        self.downloads_dir = downloads;
        self
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for directory in [
            &self.data_dir,
            &self.local_dir,
            &self.logs_dir,
            &self.updates_dir,
            &self.downloads_dir,
            &self.cache_dir,
        ] {
            std::fs::create_dir_all(directory)?;
        }
        Ok(())
    }
}

pub fn load_settings(paths: &AppPaths) -> AppSettings {
    let Ok(bytes) = std::fs::read(&paths.settings_file) else {
        return default_settings();
    };
    match serde_json::from_slice::<AppSettings>(&bytes) {
        Ok(mut settings) => {
            if settings.discovery_port == 0 {
                settings.discovery_port = 47850;
            }
            settings
        }
        Err(error) => {
            tracing::warn!(%error, "设置文件损坏，使用默认设置");
            let _ = std::fs::rename(
                &paths.settings_file,
                paths.settings_file.with_extension("json.bad"),
            );
            default_settings()
        }
    }
}

fn default_settings() -> AppSettings {
    let mut settings = AppSettings::default();
    let name = nc_platform::device_name();
    if !name.trim().is_empty() {
        settings.device_name = name;
    }
    settings
}

pub fn save_settings(paths: &AppPaths, settings: &AppSettings) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    atomic_write(&paths.settings_file, &json)
}

pub fn load_or_create_identity(paths: &AppPaths) -> nc_crypto::Result<Identity> {
    Identity::load_or_create(&paths.identity_file)
}

pub fn load_or_create_certificate(
    paths: &AppPaths,
    identity: &Identity,
) -> nc_crypto::Result<DeviceCertificate> {
    if paths.certificate_file.exists() && paths.certificate_key_file.exists() {
        let cert_pem = std::fs::read_to_string(&paths.certificate_file)?;
        let key_pem = std::fs::read_to_string(&paths.certificate_key_file)?;
        if let Ok(certificate) = load_certificate(cert_pem, key_pem) {
            return Ok(certificate);
        }
        tracing::warn!("证书文件无效，重新生成");
    }
    let certificate = generate_certificate(identity)?;
    atomic_write(&paths.certificate_file, certificate.cert_pem.as_bytes())?;
    write_private(&paths.certificate_key_file, certificate.key_pem.as_bytes())?;
    Ok(certificate)
}

pub fn load_trust_store(paths: &AppPaths) -> nc_crypto::Result<TrustStore> {
    TrustStore::load(&paths.trust_file)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
