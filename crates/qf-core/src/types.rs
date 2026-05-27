//! 核心标识类型

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 任务唯一标识
pub type TaskId = Uuid;

/// 下载任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DownloadState {
    /// 等待中
    Pending,
    /// 下载中
    Downloading,
    /// 已暂停
    Paused,
    /// 校验中
    Verifying,
    /// 已完成
    Completed,
    /// 失败
    Failed,
    /// 已取消
    Cancelled,
}

/// 文件元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    /// 文件名
    pub file_name: String,
    /// 文件大小(字节),None 表示服务端未返回 Content-Length
    pub file_size: Option<u64>,
    /// MIME 类型
    pub content_type: Option<String>,
    /// 支持分片下载
    pub supports_range: bool,
    /// ETag
    pub etag: Option<String>,
    /// 最后修改时间
    pub last_modified: Option<String>,
}

/// 分片信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FragmentInfo {
    /// 分片索引
    pub index: u32,
    /// 起始字节偏移
    pub start: u64,
    /// 结束字节偏移(含)
    pub end: u64,
    /// 分片大小(字节)
    pub size: u64,
    /// 下载进度(已下载字节数)
    pub downloaded: u64,
    /// 分片校验哈希
    pub hash: Option<String>,
}
