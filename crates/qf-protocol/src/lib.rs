//! QuantumFetch 协议层:HTTP/HTTPS/QUIC/FTP
//!
//! 实现各协议的统一传输抽象:
//! - HTTP/HTTPS 客户端(基于 reqwest)
//! - QUIC 传输(基于 quinn)
//! - FTP 客户端(基于 suppaftp)
//! - 统一 Protocol trait

pub mod ftp;
pub mod http;
pub mod quic;

pub use ftp::FtpClient;
pub use http::HttpClient;
pub use quic::QuicTransport;

/// 协议模块统一测试:验证三种协议的 Protocol trait 实现一致性
#[cfg(test)]
mod protocol_tests {
    use super::*;
    use qf_core::traits::Protocol;

    /// 辅助泛型函数:验证 Protocol trait 在所有协议上的一致行为
    ///
    /// 对不可达地址调用 probe/download_range/download_full 均应返回错误。
    async fn verify_protocol_returns_error<P: Protocol>(proto: &P, url: &str) {
        let result = proto.probe(url).await;
        assert!(result.is_err(), "probe 应返回错误");

        let result = proto.download_range(url, 0, 1023).await;
        assert!(result.is_err(), "download_range 应返回错误");

        let result = proto.download_full(url).await;
        assert!(result.is_err(), "download_full 应返回错误");
    }

    #[tokio::test]
    async fn test_ftp_protocol_trait_consistency() {
        let ftp = FtpClient::new();
        // 使用不可达地址(端口 1),FTP 客户端将返回连接错误
        verify_protocol_returns_error(&ftp, "ftp://127.0.0.1:1/file.bin").await;
    }

    #[tokio::test]
    async fn test_quic_protocol_trait_consistency() {
        let quic = QuicTransport::new_insecure().await.unwrap();
        verify_protocol_returns_error(&quic, "https://example.com/file.bin").await;
    }

    #[tokio::test]
    async fn test_all_protocols_consistent_error_messages() {
        let ftp = FtpClient::new();
        let quic = QuicTransport::new_insecure().await.unwrap();

        // FTP 客户端对不可达地址返回 Network 错误
        let ftp_err = ftp.probe("ftp://127.0.0.1:1/test").await.unwrap_err();
        assert!(
            ftp_err.to_string().contains("FTP 连接失败"),
            "FTP 错误应包含连接失败信息: {ftp_err}"
        );

        // QUIC 未连接状态下返回 Protocol 错误
        let quic_err = quic.probe("https://example.com/test").await.unwrap_err();
        assert!(
            quic_err.to_string().contains("未连接"),
            "QUIC 错误应包含 '未连接',实际: {quic_err}"
        );
    }

    #[tokio::test]
    async fn test_all_protocols_return_error_variant() {
        use qf_core::QfError;

        let ftp = FtpClient::new();
        let quic = QuicTransport::new_insecure().await.unwrap();

        // FTP 对不可达地址返回 Network 变体
        let ftp_err = ftp.probe("ftp://127.0.0.1:1/test").await.unwrap_err();
        assert!(
            matches!(ftp_err, QfError::Network(_)),
            "FTP 连接失败应返回 Network 变体,实际: {ftp_err:?}"
        );

        // QUIC 返回 Protocol 变体
        let quic_err = quic.probe("https://example.com/test").await.unwrap_err();
        assert!(matches!(quic_err, QfError::Protocol(_)));
    }
}
