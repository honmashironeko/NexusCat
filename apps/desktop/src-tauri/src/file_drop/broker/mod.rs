mod client;
mod proto;
mod server;

use crate::runtime::AppState;
use std::sync::Arc;

const BROKER_ARG: &str = "--file-drop-broker";

pub(crate) fn handle_cli_mode() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let Some(index) = args.iter().position(|arg| arg == BROKER_ARG) else {
        return false;
    };
    let pipe_name = args.get(index + 1).cloned();
    let token = args.get(index + 2).cloned();
    let exit_code = match (pipe_name, token) {
        (Some(pipe_name), Some(token)) => {
            let _log_guard = init_logging();
            install_panic_hook();
            server::run(&pipe_name, &token)
        }
        _ => {
            eprintln!("文件拖放代理缺少管道名或令牌");
            2
        }
    };
    std::process::exit(exit_code);
}

pub(crate) fn start_if_elevated(state: &Arc<AppState>) {
    if !crate::elevation::is_elevated() {
        return;
    }
    client::start(Arc::clone(state));
}

pub(crate) fn show(x: i32, y: i32, width: i32, height: i32) -> bool {
    client::show(x, y, width, height)
}

pub(crate) fn hide() {
    client::hide();
}

pub(crate) fn available() -> bool {
    client::available()
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        tracing::error!(%info, "文件拖放代理发生 panic");
    }));
}

fn init_logging() -> Option<nc_observability::LogGuard> {
    let paths = crate::config::AppPaths::resolve(false).ok()?;
    nc_observability::init_logging(&paths.local_dir.join("file-drop-broker"), "info").ok()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
