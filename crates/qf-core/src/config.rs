//! 下载配置类型

use serde::{Deserialize, Serialize};

/// 下载配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    /// 下载目录
    pub download_dir: String,
    /// 最大并发分片数
    pub max_concurrent_fragments: u32,
    /// 最大重试次数
    pub max_retries: u32,
    /// 单次请求超时(秒)
    pub request_timeout_secs: u64,
    /// 是否启用校验
    pub verify_checksum: bool,
    /// 自定义 User-Agent
    pub user_agent: String,
    /// 自定义请求头
    pub headers: std::collections::HashMap<String, String>,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            download_dir: dirs()
                .map(|d| d.join("Downloads").to_string_lossy().to_string())
                .unwrap_or_else(|| ".".to_string()),
            max_concurrent_fragments: 16,
            max_retries: 3,
            request_timeout_secs: 30,
            verify_checksum: true,
            user_agent: format!("QuantumFetch/{}", env!("CARGO_PKG_VERSION")),
            headers: std::collections::HashMap::new(),
        }
    }
}

/// 连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// 单主机最大连接数
    pub max_connections_per_host: u32,
    /// 全局最大连接数
    pub max_global_connections: u32,
    /// Keep-Alive 超时(秒)
    pub keep_alive_timeout_secs: u64,
    /// 连接建立超时(秒)
    pub connect_timeout_secs: u64,
    /// 是否启用 HTTP/2
    pub enable_http2: bool,
    /// 是否启用 QUIC
    pub enable_quic: bool,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            max_connections_per_host: 16,
            max_global_connections: 256,
            keep_alive_timeout_secs: 30,
            connect_timeout_secs: 10,
            enable_http2: true,
            enable_quic: false,
        }
    }
}

/// 调度器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// 最小分片大小(字节)
    pub min_fragment_size: u64,
    /// 最大分片大小(字节)
    pub max_fragment_size: u64,
    /// 带宽预测采样间隔(秒)
    pub sampling_interval_secs: u64,
    /// EWMA 平滑因子(0.0 ~ 1.0)
    pub ewma_alpha: f64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            min_fragment_size: 1024 * 1024,      // 1MB
            max_fragment_size: 64 * 1024 * 1024, // 64MB
            sampling_interval_secs: 60,
            ewma_alpha: 0.3,
        }
    }
}

fn dirs() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
}
