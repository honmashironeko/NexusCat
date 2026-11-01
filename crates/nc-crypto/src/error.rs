pub type Result<T> = std::result::Result<T, CryptoError>;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("密钥材料无效：{0}")]
    InvalidKey(String),

    #[error("AEAD 校验失败")]
    Aead,

    #[error("重放或序号越界：{0}")]
    Replay(u64),

    #[error("证书生成失败：{0}")]
    Certificate(String),

    #[error("TLS 配置失败：{0}")]
    Tls(String),

    #[error("信任存储损坏：{0}")]
    TrustStore(String),

    #[error("输入非法：{0}")]
    InvalidInput(String),

    #[error("I/O 错误：{0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误：{0}")]
    Serde(#[from] serde_json::Error),
}
