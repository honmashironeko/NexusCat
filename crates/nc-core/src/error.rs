use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Business,
    InvalidInput,
    Infrastructure,
    Fatal,
}

#[derive(Debug, thiserror::Error)]
pub enum NexusError {
    #[error("参数错误：{0}")]
    InvalidInput(String),

    #[error("业务规则不允许：{0}")]
    Business(String),

    #[error("未找到：{0}")]
    NotFound(String),

    #[error("权限不足：{0}")]
    Permission(String),

    #[error("尚未连接：{0}")]
    NotConnected(String),

    #[error("协议错误：{0}")]
    Protocol(String),

    #[error("加密错误：{0}")]
    Crypto(String),

    #[error("网络错误：{0}")]
    Network(String),

    #[error("媒体错误：{0}")]
    Media(String),

    #[error("平台错误：{0}")]
    Platform(String),

    #[error("更新错误：{0}")]
    Update(String),

    #[error("内部错误：{0}")]
    Internal(String),

    #[error("不可恢复错误：{0}")]
    Fatal(String),

    #[error("I/O 错误：{0}")]
    Io(#[from] std::io::Error),
}

impl NexusError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::InvalidInput(_) => ErrorKind::InvalidInput,
            Self::Business(_) | Self::NotFound(_) | Self::Permission(_) | Self::NotConnected(_) => {
                ErrorKind::Business
            }
            Self::Protocol(_)
            | Self::Crypto(_)
            | Self::Network(_)
            | Self::Media(_)
            | Self::Platform(_)
            | Self::Update(_)
            | Self::Io(_) => ErrorKind::Infrastructure,
            Self::Internal(_) | Self::Fatal(_) => ErrorKind::Fatal,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self.kind(), ErrorKind::Infrastructure)
    }

    pub fn invalid(message: impl fmt::Display) -> Self {
        Self::InvalidInput(message.to_string())
    }

    pub fn platform(message: impl fmt::Display) -> Self {
        Self::Platform(message.to_string())
    }
}

pub type Result<T> = std::result::Result<T, NexusError>;

