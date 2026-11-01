#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("帧长度不足：需要 {needed} 字节，实际 {actual}")]
    Truncated { needed: usize, actual: usize },

    #[error("帧超过上限：{actual} > {limit} 字节")]
    FrameTooLarge { actual: usize, limit: usize },

    #[error("未知帧类型：{0}")]
    UnknownFrameKind(u8),

    #[error("序列化失败：{0}")]
    Serialize(String),

    #[error("反序列化失败：{0}")]
    Deserialize(String),

    #[error("协议版本不兼容：对端 {peer}，本机 {local}")]
    VersionMismatch { peer: u16, local: u16 },

    #[error("非法消息：{0}")]
    Invalid(String),
}

impl From<bincode::Error> for ProtocolError {
    fn from(error: bincode::Error) -> Self {
        Self::Deserialize(error.to_string())
    }
}
