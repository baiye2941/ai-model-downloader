//! HTTP/HTTPS 协议实现
//!
//! 基于 reqwest 的 HTTP 客户端,支持:
//! - Range 请求(分片下载)
//! - HEAD 探测(文件元数据)
//! - Keep-Alive 连接复用

use std::net::ToSocketAddrs;
use std::pin::Pin;

use bytes::Bytes;
use futures::StreamExt;
use qf_core::filename::extract_filename;
use qf_core::traits::Protocol;
use qf_core::types::FileMetadata;
use qf_core::{ByteStream, QfError, QfResult};
use reqwest::Client;
use tracing::{debug, info, warn};

/// HTTP/HTTPS 协议客户端
pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    /// 创建新的 HTTP 客户端
    pub fn new() -> QfResult<Self> {
        let client = Client::builder()
            .user_agent(qf_core::config::USER_AGENT)
            .pool_max_idle_per_host(16)
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .no_proxy()
            .dns_resolver(std::sync::Arc::new(PublicDnsResolver))
            .redirect(safe_redirect_policy())
            .build()
            .map_err(|e| QfError::Network(format!("创建 HTTP 客户端失败: {e}")))?;
        Ok(Self { client })
    }

    /// 使用自定义 reqwest Client 创建
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// 获取内部 reqwest Client 引用
    pub fn inner(&self) -> &Client {
        &self.client
    }
}

// Default 实现已移除 — TLS 初始化可能失败,请使用 HttpClient::new()

#[derive(Debug)]
struct PublicDnsResolver;

impl reqwest::dns::Resolve for PublicDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> = (host.as_str(), 0)
                .to_socket_addrs()?
                .map(|mut addr| {
                    addr.set_port(0);
                    addr
                })
                .collect();
            for addr in &addrs {
                qf_core::reject_forbidden_ip(addr.ip())
                    .map_err(|err| -> Box<dyn std::error::Error + Send + Sync> { Box::new(err) })?;
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn validate_redirect_target(url: &reqwest::Url) -> QfResult<()> {
    qf_core::validate_public_http_url(url)
}

fn safe_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("重定向次数超过 10 次");
        }
        if let Err(err) = validate_redirect_target(attempt.url()) {
            return attempt.error(err.to_string());
        }
        attempt.follow()
    })
}

impl Protocol for HttpClient {
    fn probe(
        &self,
        url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = QfResult<FileMetadata>> + Send>> {
        let client = self.client.clone();
        let url = url.to_owned();
        Box::pin(async move {
            let parsed_url = reqwest::Url::parse(&url)?;
            qf_core::validate_public_http_url(&parsed_url)?;
            debug!(url = %qf_core::redact_url_for_log(&url), "HTTP HEAD 探测开始");
            let response = client.head(&url).send().await.map_err(|e| {
                warn!(url = %qf_core::redact_url_for_log(&url), error = %e, "HEAD 请求连接失败");
                QfError::Network(format!("HEAD 请求失败: {e}"))
            })?;

            let status = response.status();
            if !status.is_success() {
                warn!(url = %qf_core::redact_url_for_log(&url), status = %status, "HEAD 请求返回非成功状态码");
                return Err(QfError::Protocol(format!("HTTP {status}")));
            }

            let headers = response.headers();
            let content_disposition = headers
                .get("content-disposition")
                .and_then(|v| v.to_str().ok());
            let file_name = extract_filename(&url, content_disposition);
            let file_size = headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let content_type = headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string());
            let supports_range = headers
                .get("accept-ranges")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.contains("bytes"))
                .unwrap_or(false);
            let etag = headers
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string());
            let last_modified = headers
                .get("last-modified")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string());

            info!(
                url = %qf_core::redact_url_for_log(&url),
                file_size = ?file_size,
                supports_range = supports_range,
                content_type = ?content_type,
                "HTTP HEAD 探测完成"
            );

            Ok(FileMetadata {
                file_name,
                file_size,
                content_type,
                supports_range,
                etag,
                last_modified,
            })
        })
    }

    fn download_range(
        &self,
        url: &str,
        start: u64,
        end: u64,
    ) -> Pin<Box<dyn std::future::Future<Output = QfResult<Bytes>> + Send>> {
        let client = self.client.clone();
        let url = url.to_owned();
        Box::pin(async move {
            let parsed_url = reqwest::Url::parse(&url)?;
            qf_core::validate_public_http_url(&parsed_url)?;
            let range = format!("bytes={start}-{end}");
            debug!(url = %qf_core::redact_url_for_log(&url), start, end, "HTTP Range 请求开始");
            let response = client
                .get(&url)
                .header("Range", &range)
                .send()
                .await
                .map_err(|e| {
                    warn!(url = %qf_core::redact_url_for_log(&url), start, end, error = %e, "Range 请求连接失败");
                    QfError::Network(format!("Range 请求失败: {e}"))
                })?;

            let status = response.status();
            if status == reqwest::StatusCode::OK {
                warn!(url = %qf_core::redact_url_for_log(&url), "服务器忽略 Range 头,返回 HTTP 200");
                return Err(QfError::Protocol(
                    "服务器忽略 Range 头,返回 HTTP 200(不支持分片下载)".into(),
                ));
            }
            if status != reqwest::StatusCode::PARTIAL_CONTENT {
                warn!(url = %qf_core::redact_url_for_log(&url), status = %status, "Range 请求返回非预期状态码");
                return Err(QfError::Protocol(format!("HTTP {status}")));
            }

            let bytes = response
                .bytes()
                .await
                .map_err(|e| QfError::Network(format!("读取响应体失败: {e}")))?;

            info!(
                url = %qf_core::redact_url_for_log(&url),
                start,
                end,
                bytes = bytes.len(),
                "HTTP Range 下载完成"
            );
            Ok(bytes)
        })
    }

    fn download_range_stream(
        &self,
        url: &str,
        start: u64,
        end: u64,
    ) -> Pin<Box<dyn std::future::Future<Output = QfResult<ByteStream>> + Send>> {
        let client = self.client.clone();
        let url = url.to_owned();
        Box::pin(async move {
            let parsed_url = reqwest::Url::parse(&url)?;
            qf_core::validate_public_http_url(&parsed_url)?;
            let range = format!("bytes={start}-{end}");
            debug!(url = %qf_core::redact_url_for_log(&url), start, end, "HTTP 流式 Range 请求开始");
            let response = client
                .get(&url)
                .header("Range", range)
                .send()
                .await
                .map_err(|e| {
                    warn!(url = %qf_core::redact_url_for_log(&url), start, end, error = %e, "流式 Range 请求连接失败");
                    QfError::Network(format!("Range 请求失败: {e}"))
                })?;

            let status = response.status();
            if status == reqwest::StatusCode::OK {
                warn!(url = %qf_core::redact_url_for_log(&url), "服务器忽略 Range 头,返回 HTTP 200");
                return Err(QfError::Protocol(
                    "服务器忽略 Range 头,返回 HTTP 200(不支持分片下载)".into(),
                ));
            }
            if status != reqwest::StatusCode::PARTIAL_CONTENT {
                warn!(url = %qf_core::redact_url_for_log(&url), status = %status, "流式 Range 请求返回非预期状态码");
                return Err(QfError::Protocol(format!("HTTP {status}")));
            }

            info!(url = %qf_core::redact_url_for_log(&url), start, end, "HTTP 流式 Range 响应头已接收,开始流式传输");

            // 使用 bytes_stream() 获取真正的数据流,
            // 调用方通过 StreamExt::next() 逐块消费,峰值内存仅包含单个 chunk
            let stream = response.bytes_stream().map(|result| {
                result.map_err(|e| QfError::Network(format!("读取响应流数据失败: {e}")))
            });

            Ok(Box::pin(stream) as ByteStream)
        })
    }

    fn download_full(
        &self,
        url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = QfResult<Bytes>> + Send>> {
        let client = self.client.clone();
        let url = url.to_owned();
        Box::pin(async move {
            let parsed_url = reqwest::Url::parse(&url)?;
            qf_core::validate_public_http_url(&parsed_url)?;
            let response = client
                .get(&url)
                .send()
                .await
                .map_err(|e| QfError::Network(format!("下载请求失败: {e}")))?;

            let status = response.status();
            if !status.is_success() {
                return Err(QfError::Protocol(format!("HTTP {status}")));
            }

            response
                .bytes()
                .await
                .map_err(|e| QfError::Network(format!("读取响应体失败: {e}")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qf_core::filename::parse_content_disposition;

    #[test]
    fn test_extract_filename_from_url() {
        assert_eq!(
            extract_filename("http://example.com/path/file.zip", None),
            "file.zip"
        );
    }

    #[test]
    fn test_extract_filename_from_url_root() {
        assert_eq!(extract_filename("http://example.com/", None), "unknown");
    }

    #[test]
    fn test_parse_content_disposition_filename() {
        assert_eq!(
            parse_content_disposition("attachment; filename=\"test.zip\""),
            Some("test.zip".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_no_quotes() {
        assert_eq!(
            parse_content_disposition("attachment; filename=test.zip"),
            Some("test.zip".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_empty() {
        assert_eq!(parse_content_disposition(""), None);
    }

    #[test]
    fn test_http_client_creation() {
        let client = HttpClient::new();
        assert!(client.is_ok());
    }

    #[test]
    fn test_http_client_new() {
        let _client = HttpClient::new().unwrap();
    }

    #[test]
    fn test_redirect_target_validation_rejects_loopback() {
        let target = reqwest::Url::parse("http://127.0.0.1/admin").unwrap();
        assert!(super::validate_redirect_target(&target).is_err());
    }

    #[test]
    fn test_redirect_target_validation_accepts_public() {
        let target = reqwest::Url::parse("https://example.com/file.bin").unwrap();
        assert!(super::validate_redirect_target(&target).is_ok());
    }

    #[tokio::test]
    async fn test_public_dns_resolver_rejects_localhost() {
        let resolver = super::PublicDnsResolver;
        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        let result = reqwest::dns::Resolve::resolve(&resolver, name).await;
        assert!(result.is_err());
    }
}
