//! 统一错误类型

use thiserror::Error;

/// QuantumFetch 全局错误类型
#[derive(Error, Debug)]
pub enum QfError {
    #[error("网络错误: {0}")]
    Network(String),

    #[error("协议错误: {0}")]
    Protocol(String),

    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("分片错误: {0}")]
    Fragment(String),

    #[error("校验失败: 预期 {expected}, 实际 {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("配置错误: {0}")]
    Config(String),

    #[error("任务已取消")]
    Cancelled,

    #[error("任务不存在: {0}")]
    TaskNotFound(String),

    #[error("连接池已耗尽")]
    ConnectionPoolExhausted,

    #[error("超时: {0}")]
    Timeout(String),

    #[error("URL 解析错误: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("序列化错误: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("其他错误: {0}")]
    Other(String),
}

/// 统一 Result 别名
pub type QfResult<T> = Result<T, QfError>;
