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
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl From<String> for QfError {
    fn from(s: String) -> Self {
        QfError::Other(s.into())
    }
}

impl From<&str> for QfError {
    fn from(s: &str) -> Self {
        QfError::Other(s.to_string().into())
    }
}

/// 统一 Result 别名
pub type QfResult<T> = Result<T, QfError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_error_display() {
        let err = QfError::Network("连接超时".into());
        assert_eq!(err.to_string(), "网络错误: 连接超时");
    }

    #[test]
    fn test_protocol_error_display() {
        let err = QfError::Protocol("404 Not Found".into());
        assert_eq!(err.to_string(), "协议错误: 404 Not Found");
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "文件不存在");
        let err: QfError = io_err.into();
        assert!(err.to_string().contains("I/O 错误"));
    }

    #[test]
    fn test_checksum_mismatch_display() {
        let err = QfError::ChecksumMismatch {
            expected: "abc".into(),
            actual: "def".into(),
        };
        assert!(err.to_string().contains("abc"));
        assert!(err.to_string().contains("def"));
    }

    #[test]
    fn test_cancelled_display() {
        let err = QfError::Cancelled;
        assert_eq!(err.to_string(), "任务已取消");
    }

    #[test]
    fn test_task_not_found_display() {
        let err = QfError::TaskNotFound("task-123".into());
        assert!(err.to_string().contains("task-123"));
    }

    #[test]
    fn test_connection_pool_exhausted() {
        let err = QfError::ConnectionPoolExhausted;
        assert_eq!(err.to_string(), "连接池已耗尽");
    }

    #[test]
    fn test_timeout_display() {
        let err = QfError::Timeout("30s".into());
        assert!(err.to_string().contains("30s"));
    }

    #[test]
    fn test_url_parse_error_from() {
        let err: QfError = url::ParseError::EmptyHost.into();
        assert!(err.to_string().contains("URL 解析错误"));
    }

    #[test]
    fn test_serde_json_error_from() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err: QfError = json_err.into();
        assert!(err.to_string().contains("序列化错误"));
    }

    #[test]
    fn test_other_error() {
        let err = QfError::Other("未知错误".into());
        assert!(err.to_string().contains("未知错误"));
    }

    #[test]
    fn test_other_error_from_string() {
        let err: QfError = "简单错误".into();
        assert!(err.to_string().contains("简单错误"));
    }

    #[test]
    fn test_other_error_from_owned_string() {
        let err: QfError = String::from("拥有错误").into();
        assert!(err.to_string().contains("拥有错误"));
    }

    #[test]
    fn test_other_error_source_chain() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "管道断裂");
        let err = QfError::Other(Box::new(io_err));
        assert!(err.to_string().contains("管道断裂"));
    }

    #[test]
    fn test_qf_result_ok() {
        let result: QfResult<i32> = Ok(42);
        assert!(matches!(result, Ok(42)));
    }

    #[test]
    fn test_qf_result_err() {
        let result: QfResult<i32> = Err(QfError::Cancelled);
        assert!(result.is_err());
    }
}
