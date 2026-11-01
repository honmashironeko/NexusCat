mod agent;
mod config;
mod frame;
mod identity;
mod install;
mod pipe;
mod server;
mod service;

use crate::ipc::InputServiceStatusView;
use nc_core::InputEvent;
use std::sync::OnceLock;

static STARTED: OnceLock<()> = OnceLock::new();

pub fn handle_cli_mode() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == config::SERVICE_ARG) {
        service::run_service();
        return true;
    }
    if args.iter().any(|arg| arg == config::AGENT_ARG) {
        agent::run_agent();
        return true;
    }
    if args.iter().any(|arg| arg == config::INSTALL_ARG) {
        std::process::exit(install::run_install());
    }
    if args.iter().any(|arg| arg == config::UNINSTALL_ARG) {
        std::process::exit(install::run_uninstall());
    }
    false
}

pub fn start() {
    STARTED.get_or_init(server::start);
}

pub fn available() -> bool {
    server::is_connected()
}

pub fn inject(event: &InputEvent) -> bool {
    server::send(event)
}

pub fn install_elevated() -> Result<(), String> {
    let owner_sid =
        identity::current_user_sid().ok_or_else(|| "无法读取当前用户 SID".to_string())?;
    install::launch_elevated(&format!("{} {}", config::INSTALL_ARG, owner_sid))
}

pub fn uninstall_elevated() -> Result<(), String> {
    install::launch_elevated(config::UNINSTALL_ARG)
}

pub fn status() -> InputServiceStatusView {
    let config = config::load_config().ok();
    let installed = config.is_some() && config::installed_service_path().exists();
    let running = installed && install::is_running();
    let needs_update = config
        .as_ref()
        .map(|config| !config.is_current())
        .unwrap_or(false);
    let message = if !installed {
        Some("输入服务未安装".into())
    } else if !running {
        Some("输入服务未运行".into())
    } else if needs_update {
        Some("输入服务版本需要更新".into())
    } else if !server::is_connected() {
        Some("等待 SYSTEM 输入代理连接".into())
    } else {
        None
    };
    InputServiceStatusView {
        installed,
        running,
        connected: server::is_connected(),
        version: config.as_ref().map(|config| config.version.clone()),
        needs_update,
        message,
    }
}
