use std::fs;
use std::io;
use std::path::Path;

use nc_core::{NexusError, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub struct LogGuard {
    _worker_guard: WorkerGuard,
}

pub fn init_logging(data_dir: &Path, level: &str) -> Result<LogGuard> {
    let log_dir = data_dir.join("logs");
    fs::create_dir_all(&log_dir).map_err(|error| {
        NexusError::Io(io::Error::new(
            error.kind(),
            format!("创建日志目录 {} 失败：{error}", log_dir.display()),
        ))
    })?;

    let filter = EnvFilter::try_new(level)
        .map_err(|error| NexusError::InvalidInput(format!("日志级别 `{level}` 无效：{error}")))?;

    let file_appender = tracing_appender::rolling::daily(&log_dir, "nexuscat.log");
    let (file_writer, worker_guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_writer)
        .with_ansi(false);
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_ansi(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .map_err(|error| NexusError::Internal(format!("日志订阅器初始化失败：{error}")))?;

    Ok(LogGuard {
        _worker_guard: worker_guard,
    })
}

