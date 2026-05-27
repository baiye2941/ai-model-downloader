//! 核心 trait 定义
//!
//! 所有 crate 共享的公共接口抽象

use bytes::Bytes;

use crate::config::DownloadConfig;
use crate::error::QfResult;
use crate::types::{FileMetadata, FragmentInfo, TaskId};

/// 协议层 trait:负责与远程服务器通信
pub trait Protocol: Send + Sync {
    /// 探测远程文件元数据(大小、是否支持 Range 等)
    fn probe(&self, url: &str) -> impl std::future::Future<Output = QfResult<FileMetadata>> + Send;

    /// 下载指定字节范围的数据
    fn download_range(
        &self,
        url: &str,
        start: u64,
        end: u64,
    ) -> impl std::future::Future<Output = QfResult<Bytes>> + Send;

    /// 下载整个文件(不支持 Range 时使用)
    fn download_full(&self, url: &str)
    -> impl std::future::Future<Output = QfResult<Bytes>> + Send;
}

/// 存储层 trait:负责数据持久化
pub trait Storage: Send + Sync {
    /// 写入数据到指定偏移位置
    fn write_at(
        &self,
        offset: u64,
        data: &[u8],
    ) -> impl std::future::Future<Output = QfResult<usize>> + Send;

    /// 从指定偏移读取数据
    fn read_at(
        &self,
        offset: u64,
        buf: &mut [u8],
    ) -> impl std::future::Future<Output = QfResult<usize>> + Send;

    /// 将数据同步到磁盘
    fn sync(&self) -> impl std::future::Future<Output = QfResult<()>> + Send;

    /// 预分配文件空间
    fn allocate(&self, size: u64) -> impl std::future::Future<Output = QfResult<()>> + Send;

    /// 获取当前文件大小
    fn file_size(&self) -> impl std::future::Future<Output = QfResult<u64>> + Send;
}

/// 校验层 trait:负责数据完整性校验
pub trait Verifier: Send + Sync {
    /// 计算数据的哈希值
    fn compute_hash(&self, data: &[u8]) -> QfResult<String>;

    /// 校验数据是否匹配预期哈希
    fn verify(&self, data: &[u8], expected_hash: &str) -> QfResult<bool> {
        let actual = self.compute_hash(data)?;
        Ok(actual == expected_hash)
    }
}

/// 下载任务 trait:表示一个完整的下载任务
pub trait DownloadTask: Send + Sync {
    /// 获取任务 ID
    fn id(&self) -> TaskId;

    /// 获取下载 URL
    fn url(&self) -> &str;

    /// 获取下载配置
    fn config(&self) -> &DownloadConfig;

    /// 获取文件元数据
    fn metadata(&self) -> QfResult<&FileMetadata>;

    /// 获取所有分片信息
    fn fragments(&self) -> &[FragmentInfo];

    /// 获取当前状态
    fn state(&self) -> crate::types::DownloadState;

    /// 计算总体下载进度(0.0 ~ 1.0)
    fn progress(&self) -> f64 {
        let fragments = self.fragments();
        if fragments.is_empty() {
            return 0.0;
        }
        let total: u64 = fragments.iter().map(|f| f.size).sum();
        if total == 0 {
            return 1.0;
        }
        let downloaded: u64 = fragments.iter().map(|f| f.downloaded).sum();
        downloaded as f64 / total as f64
    }
}

/// 分片下载 trait:单个分片的下载操作
pub trait FragmentDownloader: Send + Sync {
    /// 下载单个分片
    fn download(
        &self,
        task_id: TaskId,
        fragment: &FragmentInfo,
    ) -> impl std::future::Future<Output = QfResult<Bytes>> + Send;

    /// 取消分片下载
    fn cancel(&self, task_id: TaskId, fragment_index: u32) -> QfResult<()>;
}
