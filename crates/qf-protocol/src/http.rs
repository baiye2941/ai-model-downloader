//! HTTP/HTTPS 协议实现
//!
//! 基于 reqwest 的 HTTP 客户端,支持:
//! - Range 请求(分片下载)
//! - HEAD 探测(文件元数据)
//! - Keep-Alive 连接复用

use bytes::Bytes;
use qf_core::traits::Protocol;
use qf_core::types::FileMetadata;
use qf_core::{QfError, QfResult};
use reqwest::Client;

/// HTTP/HTTPS 协议客户端
pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    /// 创建新的 HTTP 客户端
    pub fn new() -> QfResult<Self> {
        let client = Client::builder()
            .user_agent(format!("QuantumFetch/{}", env!("CARGO_PKG_VERSION")))
            .pool_max_idle_per_host(16)
            .tcp_keepalive(std::time::Duration::from_secs(30))
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

impl Default for HttpClient {
    fn default() -> Self {
        Self::new().expect("创建默认 HTTP 客户端失败")
    }
}

impl Protocol for HttpClient {
    async fn probe(&self, url: &str) -> QfResult<FileMetadata> {
        let response = self
            .client
            .head(url)
            .send()
            .await
            .map_err(|e| QfError::Network(format!("HEAD 请求失败: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(QfError::Protocol(format!("HTTP {status}")));
        }

        let headers = response.headers();
        let file_name = extract_filename(url, headers);
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

        Ok(FileMetadata {
            file_name,
            file_size,
            content_type,
            supports_range,
            etag,
            last_modified,
        })
    }

    async fn download_range(&self, url: &str, start: u64, end: u64) -> QfResult<Bytes> {
        let range = format!("bytes={start}-{end}");
        let response = self
            .client
            .get(url)
            .header("Range", range)
            .send()
            .await
            .map_err(|e| QfError::Network(format!("Range 请求失败: {e}")))?;

        let status = response.status();
        if !status.is_success() && status.as_u16() != 206 {
            return Err(QfError::Protocol(format!("HTTP {status}")));
        }

        response
            .bytes()
            .await
            .map_err(|e| QfError::Network(format!("读取响应体失败: {e}")))
    }

    async fn download_full(&self, url: &str) -> QfResult<Bytes> {
        let response = self
            .client
            .get(url)
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
    }
}

/// 从 URL 和 headers 提取文件名
fn extract_filename(url: &str, headers: &reqwest::header::HeaderMap) -> String {
    // 优先从 Content-Disposition 提取
    if let Some(cd) = headers.get("content-disposition") {
        if let Ok(cd_str) = cd.to_str() {
            if let Some(name) = parse_content_disposition(cd_str) {
                return name;
            }
        }
    }
    // 从 URL 路径提取
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path()
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "download".to_string())
}

/// 解析 Content-Disposition 头中的文件名
fn parse_content_disposition(value: &str) -> Option<String> {
    // filename="name"
    if let Some(pos) = value.find("filename=") {
        let rest = &value[pos + 9..];
        let name = rest.trim_matches(|c| c == '"' || c == '\'');
        let name = name.split(';').next().unwrap_or(name).trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_filename_from_url() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(
            extract_filename("http://example.com/path/file.zip", &headers),
            "file.zip"
        );
    }

    #[test]
    fn test_extract_filename_from_url_root() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(
            extract_filename("http://example.com/", &headers),
            "download"
        );
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
    fn test_http_client_default() {
        let _client = HttpClient::default();
    }
}
