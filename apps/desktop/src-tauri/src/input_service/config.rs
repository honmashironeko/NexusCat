use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SERVICE_NAME: &str = "NexusCatInputService";
pub const SERVICE_DISPLAY_NAME: &str = "NexusCat Input Service";
pub const SERVICE_ARG: &str = "--input-service";
pub const AGENT_ARG: &str = "--input-agent";
pub const INSTALL_ARG: &str = "--install-input-service";
pub const UNINSTALL_ARG: &str = "--uninstall-input-service";
pub const PIPE_NAME: &str = r"\\.\pipe\NexusCat.InputAgent.v1";
pub const SERVICE_EXE_NAME: &str = "NexusCatInputService.exe";
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub version: String,
    pub protocol_version: u32,
    pub token: String,
    pub user_sid: String,
    pub app_path: String,
    pub service_path: String,
    pub pipe_name: String,
}

impl AgentConfig {
    pub fn is_current(&self) -> bool {
        self.version == env!("CARGO_PKG_VERSION")
            && self.protocol_version == PROTOCOL_VERSION
            && service_binary_matches_app()
    }
}

fn service_binary_matches_app() -> bool {
    let Ok(installed) = std::fs::metadata(installed_service_path()) else {
        return false;
    };
    let Ok(current) = std::env::current_exe().and_then(std::fs::metadata) else {
        return false;
    };
    installed.len() == current.len()
}

pub fn program_data_dir() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("NexusCat")
}

pub fn config_path() -> PathBuf {
    program_data_dir().join("input-service.json")
}

pub fn log_path() -> PathBuf {
    program_data_dir().join("input-service.log")
}

pub fn program_files_dir() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
        .join("NexusCat")
}

pub fn installed_service_path() -> PathBuf {
    program_files_dir().join(SERVICE_EXE_NAME)
}

pub fn load_config() -> Result<AgentConfig, String> {
    let path = config_path();
    let data = fs::read(&path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    serde_json::from_slice(&data).map_err(|error| format!("解析 {} 失败：{error}", path.display()))
}

pub fn save_config(config: &AgentConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败：{error}", parent.display()))?;
    }
    let data = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
    fs::write(&path, data).map_err(|error| format!("写入 {} 失败：{error}", path.display()))
}

pub fn remove_config() {
    let _ = fs::remove_file(config_path());
}

pub fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn log(message: impl AsRef<str>) {
    let path = log_path();
    let _ = fs::create_dir_all(program_data_dir());
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[{timestamp}] {}", message.as_ref());
    }
}
