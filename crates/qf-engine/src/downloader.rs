//! 下载任务执行器
//!
//! 将协议层、I/O 层、校验层串联为完整的下载编排流程:
//! 1. `probe`  -- 探测文件元数据
//! 2. `plan`   -- 规划分片
//! 3. `prepare_storage` -- 预分配文件空间
//! 4. `execute` -- 并发下载全部分片
//! 5. `verify`  -- 校验完整性
//!
//! `run()` 方法一键执行上述全部步骤。

use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use qf_core::config::DownloadConfig;
#[cfg(test)]
use qf_core::traits::Storage;
use qf_core::traits::{Protocol, Verifier};
use qf_core::types::{DownloadState, FileMetadata, FragmentInfo, TaskId};
use qf_core::{QfError, QfResult};
use qf_crypto::cpu::CpuVerifier;
use qf_io::TokioFile;
use qf_io::storage::AsyncStorage;
use qf_protocol::http::HttpClient;

use crate::fragment::FragmentRecord;
use crate::orchestrator::DownloadOrchestrator;

// 测试模式下引入 mock 实现
#[cfg(test)]
use qf_core::test_harness::harness::MemoryStorage as MemStorage;
#[cfg(test)]
use qf_core::test_harness::harness::MockProtocol as MockProto;

// ---------------------------------------------------------------------------
// ProtocolKind: 协议类型枚举
// ---------------------------------------------------------------------------

/// 协议类型枚举
///
/// `Protocol` trait 使用 RPITIT,不满足 object-safe 条件,
/// 因此通过 enum 实现静态分发。后续可扩展 QUIC、FTP 等变体。
pub enum ProtocolKind {
    /// HTTP/HTTPS 协议
    Http(HttpClient),
    #[cfg(test)]
    /// Mock 协议,仅在测试模式下可用
    Mock(MockProto),
}

// reqwest::Client 内部是 Arc,clone 开销极低
impl Clone for ProtocolKind {
    fn clone(&self) -> Self {
        match self {
            ProtocolKind::Http(c) => ProtocolKind::Http(HttpClient::with_client(c.inner().clone())),
            #[cfg(test)]
            ProtocolKind::Mock(m) => ProtocolKind::Mock(m.clone()),
        }
    }
}

impl ProtocolKind {
    /// 根据 URL scheme 自动选择协议
    fn from_url(url: &str) -> QfResult<Self> {
        if url.starts_with("http://") || url.starts_with("https://") {
            Ok(ProtocolKind::Http(HttpClient::new()?))
        } else {
            #[cfg(test)]
            {
                // 测试模式下对未知 scheme 也返回 Http,由 probe 阶段报错
                Ok(ProtocolKind::Http(HttpClient::new()?))
            }
            #[cfg(not(test))]
            {
                Err(QfError::Config(format!("不支持的协议: {url}")))
            }
        }
    }

    /// 探测文件元数据
    async fn probe(&self, url: &str) -> QfResult<FileMetadata> {
        match self {
            ProtocolKind::Http(c) => c.probe(url).await,
            #[cfg(test)]
            ProtocolKind::Mock(c) => c.probe(url).await,
        }
    }

    /// 下载数据:有 range 时使用 Range 请求,无 range 时整块下载
    async fn download(&self, url: &str, range: Option<(u64, u64)>) -> QfResult<Bytes> {
        match self {
            ProtocolKind::Http(c) => {
                if let Some((start, end)) = range {
                    c.download_range(url, start, end).await
                } else {
                    c.download_full(url).await
                }
            }
            #[cfg(test)]
            ProtocolKind::Mock(c) => {
                if let Some((start, end)) = range {
                    c.download_range(url, start, end).await
                } else {
                    c.download_full(url).await
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// StorageKind: 存储类型枚举
// ---------------------------------------------------------------------------

/// 存储类型枚举
///
/// 封装不同存储后端,统一提供读写接口。后续可扩展 io_uring 等变体。
/// 内部使用 `Arc` 包装以支持克隆(共享同一底层存储)。
pub enum StorageKind {
    /// 基于 tokio 的异步文件存储
    Tokio(Arc<TokioFile>),
    #[cfg(test)]
    /// 内存存储,仅在测试模式下可用
    Memory(Arc<MemStorage>),
}

impl Clone for StorageKind {
    fn clone(&self) -> Self {
        match self {
            StorageKind::Tokio(s) => StorageKind::Tokio(Arc::clone(s)),
            #[cfg(test)]
            StorageKind::Memory(s) => StorageKind::Memory(Arc::clone(s)),
        }
    }
}

impl StorageKind {
    /// 打开或创建存储
    ///
    /// - `Tokio`: 在 `path` 创建/打开文件
    /// - `Memory`: 创建空内存缓冲区(测试用)
    async fn open(path: &std::path::Path) -> QfResult<Self> {
        let storage = TokioFile::open(path).await?;
        Ok(StorageKind::Tokio(Arc::new(storage)))
    }

    /// 创建内存存储(测试辅助)
    #[cfg(test)]
    fn memory() -> Self {
        StorageKind::Memory(Arc::new(MemStorage::new()))
    }

    /// 创建指定容量的内存存储(测试辅助)
    #[cfg(test)]
    fn memory_with_capacity(cap: usize) -> Self {
        StorageKind::Memory(Arc::new(MemStorage::with_capacity(cap)))
    }

    /// 写入数据到指定偏移
    async fn write_at(&self, offset: u64, data: &[u8]) -> QfResult<usize> {
        match self {
            StorageKind::Tokio(s) => s.write_at(offset, data).await,
            #[cfg(test)]
            StorageKind::Memory(s) => s.write_at(offset, data).await,
        }
    }

    /// 从指定偏移读取数据
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> QfResult<usize> {
        match self {
            StorageKind::Tokio(s) => s.read_at(offset, buf).await,
            #[cfg(test)]
            StorageKind::Memory(s) => s.read_at(offset, buf).await,
        }
    }

    /// 预分配文件空间
    async fn allocate(&self, size: u64) -> QfResult<()> {
        match self {
            StorageKind::Tokio(s) => s.allocate(size).await,
            #[cfg(test)]
            StorageKind::Memory(s) => s.allocate(size).await,
        }
    }

    /// 同步数据到磁盘
    async fn sync(&self) -> QfResult<()> {
        match self {
            StorageKind::Tokio(s) => s.sync().await,
            #[cfg(test)]
            StorageKind::Memory(s) => s.sync().await,
        }
    }
}

// ---------------------------------------------------------------------------
// VerifierKind: 校验器类型枚举
// ---------------------------------------------------------------------------

/// 校验器类型枚举
///
/// 封装不同的哈希校验实现。后续可扩展 GPU 校验等变体。
pub enum VerifierKind {
    /// CPU 校验器(blake3/sha256)
    Cpu(CpuVerifier),
}

// CpuVerifier 是无状态的,创建新实例等价于 clone
impl Clone for VerifierKind {
    fn clone(&self) -> Self {
        match self {
            VerifierKind::Cpu(_) => VerifierKind::Cpu(CpuVerifier::blake3()),
        }
    }
}

impl VerifierKind {
    /// 创建默认 CPU 校验器(blake3)
    fn blake3() -> Self {
        VerifierKind::Cpu(CpuVerifier::blake3())
    }

    /// 校验数据是否匹配预期哈希
    fn verify(&self, data: &[u8], expected: &str) -> QfResult<bool> {
        match self {
            VerifierKind::Cpu(v) => v.verify(data, expected),
        }
    }
}

// ---------------------------------------------------------------------------
// DownloadTask: 下载任务执行器
// ---------------------------------------------------------------------------

/// 单个下载任务的执行器
///
/// 串联协议层、存储层、校验层,提供完整的下载编排流程。
pub struct DownloadTask {
    /// 任务 ID
    pub id: TaskId,
    /// 下载 URL
    pub url: String,
    /// 下载配置
    pub config: DownloadConfig,
    /// 协议客户端
    protocol: ProtocolKind,
    /// 存储后端
    storage: Arc<StorageKind>,
    /// 校验器
    verifier: VerifierKind,
    /// 编排器(分片策略与带宽追踪)
    orchestrator: DownloadOrchestrator,
    /// 当前状态
    state: DownloadState,
    /// 文件元数据(探测后填充)
    metadata: Option<FileMetadata>,
    /// 分片记录(规划后填充)
    fragments: Vec<FragmentRecord>,
}

impl DownloadTask {
    /// 创建新的下载任务
    ///
    /// 根据 URL scheme 自动选择协议后端,使用默认 blake3 校验器。
    /// 存储文件位于 `config.download_dir` 目录下,文件名在 `probe` 阶段确定。
    pub async fn new(url: String, config: DownloadConfig) -> QfResult<Self> {
        // 解析 URL 验证合法性
        let _parsed = url::Url::parse(&url)?;

        let protocol = ProtocolKind::from_url(&url)?;
        let storage_path = std::path::Path::new(&config.download_dir).join("qf_temp_download");
        let storage = Arc::new(StorageKind::open(&storage_path).await?);

        Ok(Self {
            id: TaskId::new_v4(),
            url,
            config,
            protocol,
            storage,
            verifier: VerifierKind::blake3(),
            orchestrator: DownloadOrchestrator::new(Default::default()),
            state: DownloadState::Pending,
            metadata: None,
            fragments: Vec::new(),
        })
    }

    /// 使用测试辅助构造器(仅测试模式)
    ///
    /// 允许注入 mock 协议和存储,避免真实网络和文件 I/O。
    #[cfg(test)]
    fn new_for_test(
        url: String,
        config: DownloadConfig,
        protocol: ProtocolKind,
        storage: StorageKind,
    ) -> Self {
        Self {
            id: TaskId::new_v4(),
            url,
            config,
            protocol,
            storage: Arc::new(storage),
            verifier: VerifierKind::blake3(),
            orchestrator: DownloadOrchestrator::new(Default::default()),
            state: DownloadState::Pending,
            metadata: None,
            fragments: Vec::new(),
        }
    }

    // ----- 步骤 1: 探测 -----

    /// 探测文件元数据
    ///
    /// 向服务端发送 HEAD 请求,获取文件名、大小、Range 支持等信息。
    pub async fn probe(&mut self) -> QfResult<&FileMetadata> {
        info!(url = %self.url, "开始探测文件元数据");
        let metadata = self.protocol.probe(&self.url).await?;
        info!(
            file_name = %metadata.file_name,
            file_size = ?metadata.file_size,
            supports_range = metadata.supports_range,
            "探测完成"
        );
        self.metadata = Some(metadata);
        Ok(self.metadata.as_ref().expect("元数据已填充"))
    }

    // ----- 步骤 2: 规划分片 -----

    /// 根据已探测的文件元数据规划分片
    ///
    /// 调用编排器计算最优分片策略,生成分片列表并存入内部状态。
    /// 必须在 `probe()` 之后调用。
    pub fn plan(&mut self) -> QfResult<Vec<FragmentInfo>> {
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| QfError::Config("必须先调用 probe() 获取文件元数据".into()))?;

        let file_size = metadata.file_size.unwrap_or(0);
        let fragments = self
            .orchestrator
            .plan_fragments(file_size, metadata.supports_range);

        info!(count = fragments.len(), "分片规划完成");

        self.fragments = fragments
            .iter()
            .map(|info| FragmentRecord::new(info.clone(), self.config.max_retries))
            .collect();

        Ok(fragments)
    }

    // ----- 步骤 3: 预分配存储 -----

    /// 预分配文件空间
    ///
    /// 根据文件大小在存储后端预留空间,支持分片并发写入。
    pub async fn prepare_storage(&self) -> QfResult<()> {
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| QfError::Config("必须先调用 probe() 获取文件元数据".into()))?;

        let size = metadata.file_size.unwrap_or(0);
        if size > 0 {
            self.storage.allocate(size).await?;
            debug!(size, "存储空间预分配完成");
        }
        Ok(())
    }

    // ----- 步骤 4: 并发执行下载 -----

    /// 执行全部分片下载
    ///
    /// 根据配置的最大并发数使用信号量控制并发,每个分片独立下载并写入存储。
    /// 不支持 Range 请求时退化为整块下载。
    pub async fn execute(&mut self) -> QfResult<()> {
        self.state = DownloadState::Downloading;
        info!("开始执行下载任务");

        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| QfError::Config("必须先调用 probe()".into()))?;

        let supports_range = metadata.supports_range;
        let file_size = metadata.file_size.unwrap_or(0);

        // 空文件无需下载
        if file_size == 0 {
            self.state = DownloadState::Completed;
            info!("文件大小为 0,跳过下载");
            return Ok(());
        }

        // 不支持 Range:整块下载
        if !supports_range || self.fragments.len() <= 1 {
            return self.execute_full_download().await;
        }

        // 支持 Range:并发分片下载
        self.execute_fragmented_download().await
    }

    /// 整块下载(不支持 Range 或单分片)
    async fn execute_full_download(&mut self) -> QfResult<()> {
        let data = self.protocol.download(&self.url, None).await?;
        let written = self.storage.write_at(0, &data).await?;
        debug!(written, "整块下载写入完成");

        if let Some(frag) = self.fragments.first_mut() {
            frag.info.downloaded = written as u64;
        }
        self.state = DownloadState::Completed;
        Ok(())
    }

    /// 并发分片下载
    async fn execute_fragmented_download(&mut self) -> QfResult<()> {
        let semaphore = Arc::new(Semaphore::new(
            self.config.max_concurrent_fragments as usize,
        ));
        let url = self.url.clone();
        let storage = self.storage.clone();
        let protocol = self.protocol.clone();

        // 构建分片下载任务
        let mut handles: Vec<JoinHandle<QfResult<(u32, u64)>>> = Vec::new();

        for frag in &self.fragments {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| QfError::Other(format!("信号量获取失败: {e}")))?;

            let frag_url = url.clone();
            let frag_storage = storage.clone();
            let frag_protocol = protocol.clone();
            let frag_index = frag.info.index;
            let frag_start = frag.info.start;
            let frag_end = frag.info.end;

            let handle = tokio::spawn(async move {
                let _permit = permit; // 持有许可直至任务完成

                debug!(
                    index = frag_index,
                    start = frag_start,
                    end = frag_end,
                    "开始下载分片"
                );

                let data = frag_protocol
                    .download(&frag_url, Some((frag_start, frag_end)))
                    .await?;

                let written = frag_storage.write_at(frag_start, &data).await?;

                info!(index = frag_index, written, "分片下载完成");
                Ok((frag_index, written as u64))
            });

            handles.push(handle);
        }

        // 等待全部分片完成,更新进度
        for handle in handles {
            let result = handle
                .await
                .map_err(|e| QfError::Other(format!("分片任务 panic: {e}")))?;

            let (index, downloaded) = result?;

            if let Some(frag) = self.fragments.iter_mut().find(|f| f.info.index == index) {
                frag.info.downloaded = downloaded;
                frag.state = crate::fragment::FragmentState::Done;
            }
        }

        self.storage.sync().await?;
        self.state = DownloadState::Completed;
        info!("全部分片下载完成");
        Ok(())
    }

    // ----- 步骤 5: 校验 -----

    /// 校验已下载数据的完整性
    ///
    /// 遍历所有带哈希值的分片,从存储中读取数据并与预期哈希比对。
    /// 任一分片校验失败即返回 `false`。
    pub async fn verify(&mut self) -> QfResult<bool> {
        if !self.config.verify_checksum {
            debug!("校验已禁用,跳过");
            return Ok(true);
        }

        self.state = DownloadState::Verifying;
        info!("开始校验文件完整性");

        for frag in &self.fragments {
            if let Some(ref expected_hash) = frag.info.hash {
                let mut buf = vec![0u8; frag.info.size as usize];
                let read = self.storage.read_at(frag.info.start, &mut buf).await?;
                buf.truncate(read);

                if !self.verifier.verify(&buf, expected_hash)? {
                    warn!(
                        index = frag.info.index,
                        expected = %expected_hash,
                        "分片校验失败"
                    );
                    self.state = DownloadState::Failed;
                    return Ok(false);
                }

                debug!(index = frag.info.index, "分片校验通过");
            }
        }

        info!("文件完整性校验通过");
        Ok(true)
    }

    // ----- 一键运行 -----

    /// 一键执行完整下载流程
    ///
    /// 依次执行: 探测 -> 规划 -> 预分配 -> 下载 -> 校验
    /// 任一步骤失败将标记任务为 `Failed` 并返回错误。
    pub async fn run(&mut self) -> QfResult<()> {
        info!(url = %self.url, "启动下载任务");

        let result = self.run_inner().await;

        if result.is_err() {
            self.state = DownloadState::Failed;
            warn!("下载任务失败");
        }

        result
    }

    /// 内部执行逻辑,便于 run() 统一处理错误状态
    async fn run_inner(&mut self) -> QfResult<()> {
        // 步骤 1: 探测
        self.probe().await?;

        // 步骤 2: 规划分片
        self.plan()?;

        // 步骤 3: 预分配存储
        self.prepare_storage().await?;

        // 步骤 4: 执行下载
        self.execute().await?;

        // 步骤 5: 校验
        let verified = self.verify().await?;
        if !verified {
            return Err(QfError::ChecksumMismatch {
                expected: "文件哈希".into(),
                actual: "校验失败".into(),
            });
        }

        self.state = DownloadState::Completed;
        info!("下载任务完成");
        Ok(())
    }

    // ----- 状态查询 -----

    /// 获取当前下载进度(0.0 ~ 1.0)
    pub fn progress(&self) -> f64 {
        // 已完成的任务进度为 1.0
        if self.state == DownloadState::Completed {
            return 1.0;
        }
        if self.fragments.is_empty() {
            // 无分片:如果已知文件大小为 0 则视为完成
            if let Some(ref meta) = self.metadata
                && meta.file_size == Some(0)
            {
                return 1.0;
            }
            return 0.0;
        }
        let total: u64 = self.fragments.iter().map(|f| f.info.size).sum();
        if total == 0 {
            return 1.0;
        }
        let downloaded: u64 = self.fragments.iter().map(|f| f.info.downloaded).sum();
        downloaded as f64 / total as f64
    }

    /// 获取当前状态
    pub fn state(&self) -> DownloadState {
        self.state
    }

    /// 获取文件元数据(需先调用 probe)
    pub fn metadata(&self) -> Option<&FileMetadata> {
        self.metadata.as_ref()
    }

    /// 获取分片信息(需先调用 plan)
    pub fn fragment_infos(&self) -> Vec<FragmentInfo> {
        self.fragments.iter().map(|f| f.info.clone()).collect()
    }
}

// ===========================================================================
// 测试
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use qf_core::test_harness::harness::{test_config, test_metadata};
    use qf_core::traits::Verifier as VerifierTrait;

    /// 辅助函数:创建带 mock 协议和存储的测试任务
    fn make_task(
        protocol: ProtocolKind,
        storage: StorageKind,
        config: DownloadConfig,
    ) -> DownloadTask {
        DownloadTask::new_for_test(
            "http://example.com/file.bin".into(),
            config,
            protocol,
            storage,
        )
    }

    // ------ 1. DownloadTask::new 正确初始化 -----

    #[tokio::test]
    async fn test_new_initializes_fields() {
        let config = test_config();
        let task = DownloadTask::new("http://example.com/test.bin".into(), config)
            .await
            .expect("创建任务失败");

        assert_eq!(task.state(), DownloadState::Pending);
        assert_eq!(task.url, "http://example.com/test.bin");
        assert!(task.metadata().is_none());
        assert!(task.fragment_infos().is_empty());
        assert!((task.progress() - 0.0).abs() < f64::EPSILON);
    }

    // ------ 2. probe 获取元数据 -----

    #[tokio::test]
    async fn test_probe_fetches_metadata() {
        let meta = test_metadata("data.zip", 2048);
        let protocol = ProtocolKind::Mock(MockProto::new(meta.clone()));
        let storage = StorageKind::memory();
        let mut task = make_task(protocol, storage, test_config());

        let result = task.probe().await;
        assert!(result.is_ok());

        let m = result.unwrap();
        assert_eq!(m.file_name, "data.zip");
        assert_eq!(m.file_size, Some(2048));
        assert!(m.supports_range);
    }

    #[tokio::test]
    async fn test_probe_propagates_error() {
        let protocol = ProtocolKind::Mock(MockProto::failing(QfError::Network("连接超时".into())));
        let storage = StorageKind::memory();
        let mut task = make_task(protocol, storage, test_config());

        let result = task.probe().await;
        assert!(result.is_err());
    }

    // ------ 3. plan 根据元数据生成分片 -----

    #[tokio::test]
    async fn test_plan_generates_fragments() {
        let meta = test_metadata("large.bin", 10_000);
        let protocol = ProtocolKind::Mock(MockProto::new(meta));
        let storage = StorageKind::memory();
        let mut task = make_task(protocol, storage, test_config());

        task.probe().await.unwrap();
        let frags = task.plan().unwrap();

        assert!(!frags.is_empty());
        // 所有分片覆盖完整文件
        let total: u64 = frags.iter().map(|f| f.size).sum();
        assert_eq!(total, 10_000);
        // 内部状态同步
        assert_eq!(task.fragment_infos().len(), frags.len());
    }

    #[test]
    fn test_plan_without_probe_fails() {
        let protocol = ProtocolKind::Mock(MockProto::new(test_metadata("f.bin", 100)));
        let storage = StorageKind::memory();
        let mut task = make_task(protocol, storage, test_config());

        // 未调用 probe,直接 plan 应报错
        let result = task.plan();
        assert!(result.is_err());
    }

    // ------ 4. prepare_storage 预分配空间 -----

    #[tokio::test]
    async fn test_prepare_storage_allocates() {
        let file_size = 4096u64;
        let meta = test_metadata("alloc.bin", file_size);
        let protocol = ProtocolKind::Mock(MockProto::new(meta));
        let storage = StorageKind::memory();
        let mut task = make_task(protocol, storage, test_config());

        task.probe().await.unwrap();
        task.prepare_storage().await.unwrap();

        // 验证内存存储已分配
        if let StorageKind::Memory(ref s) = *task.storage {
            assert_eq!(s.file_size().await.unwrap(), file_size);
        }
    }

    // ------ 5. 完整 run 流程(使用 mock) -----

    #[tokio::test]
    async fn test_run_full_flow_with_mock() {
        let frag_size = 334u64;
        let total_size = frag_size * 3;

        // 构造分片数据
        let frag_a = Bytes::from(vec![0xAA; frag_size as usize]);
        let frag_b = Bytes::from(vec![0xBB; frag_size as usize]);
        let frag_c = Bytes::from(vec![0xCC; frag_size as usize]);

        let meta = FileMetadata {
            file_name: "test.bin".into(),
            file_size: Some(total_size),
            content_type: None,
            supports_range: true,
            etag: None,
            last_modified: None,
        };

        let protocol = ProtocolKind::Mock(
            MockProto::new(meta)
                .with_range_data(0, frag_size - 1, frag_a.clone())
                .with_range_data(frag_size, 2 * frag_size - 1, frag_b.clone())
                .with_range_data(2 * frag_size, total_size - 1, frag_c.clone()),
        );

        let storage = StorageKind::memory_with_capacity(total_size as usize);

        // 调度器配置:确保恰好产生 3 个分片
        let sched_config = qf_core::config::SchedulerConfig {
            min_fragment_size: frag_size,
            max_fragment_size: frag_size,
            sampling_interval_secs: 60,
            ewma_alpha: 0.3,
        };
        let config = DownloadConfig {
            verify_checksum: false, // 本测试不校验哈希
            ..test_config()
        };

        let mut task = DownloadTask::new_for_test(
            "http://example.com/test.bin".into(),
            config,
            protocol,
            storage,
        );

        // 使用自定义调度器配置创建编排器
        task.orchestrator =
            DownloadOrchestrator::with_scheduler_config(Default::default(), sched_config);

        task.run().await.expect("下载流程失败");

        assert_eq!(task.state(), DownloadState::Completed);
        assert!((task.progress() - 1.0).abs() < f64::EPSILON);

        // 验证写入数据的正确性
        let mut buf = vec![0u8; total_size as usize];
        task.storage.read_at(0, &mut buf).await.unwrap();
        assert_eq!(&buf[..frag_size as usize], &frag_a[..]);
        assert_eq!(
            &buf[frag_size as usize..2 * frag_size as usize],
            &frag_b[..]
        );
        assert_eq!(&buf[2 * frag_size as usize..], &frag_c[..]);
    }

    /// 不支持 Range 请求时使用整块下载
    #[tokio::test]
    async fn test_run_no_range_support() {
        let data = Bytes::from_static(b"hello world no range");
        let meta = FileMetadata {
            file_name: "no_range.bin".into(),
            file_size: Some(data.len() as u64),
            content_type: None,
            supports_range: false,
            etag: None,
            last_modified: None,
        };

        let protocol = ProtocolKind::Mock(MockProto::new(meta).with_default_data(data.clone()));

        let storage = StorageKind::memory_with_capacity(data.len());

        let mut task = make_task(
            protocol,
            storage,
            DownloadConfig {
                verify_checksum: false,
                ..test_config()
            },
        );

        task.probe().await.unwrap();
        task.plan().unwrap();
        task.prepare_storage().await.unwrap();
        task.execute().await.unwrap();

        assert_eq!(task.state(), DownloadState::Completed);
    }

    // ------ 6. 进度追踪正确 -----

    #[test]
    fn test_progress_tracking() {
        let protocol = ProtocolKind::Mock(MockProto::new(test_metadata("p.bin", 100)));
        let storage = StorageKind::memory();
        let mut task = make_task(protocol, storage, test_config());

        // 模拟 3 个分片,部分完成
        task.fragments = vec![
            FragmentRecord::new(
                FragmentInfo {
                    index: 0,
                    start: 0,
                    end: 32,
                    size: 33,
                    downloaded: 33,
                    hash: None,
                },
                3,
            ),
            FragmentRecord::new(
                FragmentInfo {
                    index: 1,
                    start: 33,
                    end: 65,
                    size: 33,
                    downloaded: 10,
                    hash: None,
                },
                3,
            ),
            FragmentRecord::new(
                FragmentInfo {
                    index: 2,
                    start: 66,
                    end: 99,
                    size: 34,
                    downloaded: 0,
                    hash: None,
                },
                3,
            ),
        ];

        // 总大小 100,已下载 43
        let progress = task.progress();
        assert!((progress - 0.43).abs() < 0.001);
    }

    #[test]
    fn test_progress_no_fragments_is_zero() {
        let protocol = ProtocolKind::Mock(MockProto::new(test_metadata("e.bin", 100)));
        let storage = StorageKind::memory();
        let task = make_task(protocol, storage, test_config());
        assert!((task.progress() - 0.0).abs() < f64::EPSILON);
    }

    // ------ 7. 状态转换正确 -----

    #[tokio::test]
    async fn test_state_transitions() {
        let meta = test_metadata("state.bin", 100);
        let default_data = Bytes::from(vec![0u8; 100]);
        let protocol = ProtocolKind::Mock(MockProto::new(meta).with_default_data(default_data));
        let storage = StorageKind::memory_with_capacity(100);
        let mut task = make_task(protocol, storage, test_config());

        // 初始状态
        assert_eq!(task.state(), DownloadState::Pending);

        // probe 不改变状态
        task.probe().await.unwrap();
        assert_eq!(task.state(), DownloadState::Pending);

        // plan 不改变状态
        task.plan().unwrap();
        assert_eq!(task.state(), DownloadState::Pending);

        // execute 转为 Downloading,完成后转为 Completed
        task.execute().await.unwrap();
        assert_eq!(task.state(), DownloadState::Completed);
    }

    // ------ 8. 并发分片数限制 -----

    #[tokio::test]
    async fn test_concurrent_fragment_execution() {
        let total_size = 400u64;
        let frag_count = 4;
        let frag_size = total_size / frag_count;

        let meta = test_metadata("conc.bin", total_size);
        let mut protocol_mock = MockProto::new(meta);
        for i in 0..frag_count {
            let start = i * frag_size;
            let end = start + frag_size - 1;
            let data = Bytes::from(vec![(i + 1) as u8; frag_size as usize]);
            protocol_mock = protocol_mock.with_range_data(start, end, data);
        }

        let protocol = ProtocolKind::Mock(protocol_mock);
        let storage = StorageKind::memory_with_capacity(total_size as usize);
        let config = DownloadConfig {
            max_concurrent_fragments: 2, // 限制并发为 2
            verify_checksum: false,
            ..test_config()
        };

        // 使用小分片配置以产生多个分片
        let sched_config = qf_core::config::SchedulerConfig {
            min_fragment_size: 100,
            max_fragment_size: 110,
            ..Default::default()
        };

        let mut task = DownloadTask::new_for_test(
            "http://example.com/conc.bin".into(),
            config,
            protocol,
            storage,
        );
        task.orchestrator =
            DownloadOrchestrator::with_scheduler_config(Default::default(), sched_config);

        task.probe().await.unwrap();
        task.plan().unwrap();
        task.prepare_storage().await.unwrap();
        task.execute().await.unwrap();

        assert_eq!(task.state(), DownloadState::Completed);
        assert!((task.progress() - 1.0).abs() < f64::EPSILON);
    }

    // ------ 9. 分片校验 -----

    #[tokio::test]
    async fn test_verify_fragments_with_hash() {
        let data = Bytes::from_static(b"verify this data block");
        let hash = {
            let v = CpuVerifier::blake3();
            v.compute_hash(&data).unwrap()
        };

        let frag_info = FragmentInfo {
            index: 0,
            start: 0,
            end: data.len() as u64 - 1,
            size: data.len() as u64,
            downloaded: 0,
            hash: Some(hash),
        };

        let protocol =
            ProtocolKind::Mock(MockProto::new(test_metadata("v.bin", data.len() as u64)));
        let storage = StorageKind::memory_with_capacity(data.len());

        let mut task = make_task(
            protocol,
            storage,
            DownloadConfig {
                verify_checksum: true,
                ..test_config()
            },
        );

        // 手动写入数据到存储
        task.storage.write_at(0, &data).await.unwrap();

        // 设置分片记录
        task.fragments = vec![FragmentRecord::new(frag_info, 3)];
        task.metadata = Some(test_metadata("v.bin", data.len() as u64));

        let verified = task.verify().await.unwrap();
        assert!(verified, "带正确哈希的分片应通过校验");
    }

    #[tokio::test]
    async fn test_verify_detects_corruption() {
        let data = Bytes::from_static(b"original data");
        let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";

        let frag_info = FragmentInfo {
            index: 0,
            start: 0,
            end: data.len() as u64 - 1,
            size: data.len() as u64,
            downloaded: 0,
            hash: Some(wrong_hash.into()),
        };

        let protocol =
            ProtocolKind::Mock(MockProto::new(test_metadata("c.bin", data.len() as u64)));
        let storage = StorageKind::memory_with_capacity(data.len());

        let mut task = make_task(
            protocol,
            storage,
            DownloadConfig {
                verify_checksum: true,
                ..test_config()
            },
        );

        task.storage.write_at(0, &data).await.unwrap();
        task.fragments = vec![FragmentRecord::new(frag_info, 3)];
        task.metadata = Some(test_metadata("c.bin", data.len() as u64));

        let verified = task.verify().await.unwrap();
        assert!(!verified, "哈希不匹配时校验应失败");
        assert_eq!(task.state(), DownloadState::Failed);
    }

    #[tokio::test]
    async fn test_verify_skipped_when_disabled() {
        let protocol = ProtocolKind::Mock(MockProto::new(test_metadata("s.bin", 100)));
        let storage = StorageKind::memory();
        let mut task = make_task(
            protocol,
            storage,
            DownloadConfig {
                verify_checksum: false,
                ..test_config()
            },
        );

        let verified = task.verify().await.unwrap();
        assert!(verified, "校验禁用时应返回 true");
    }

    // ------ 10. 空文件处理 -----

    #[tokio::test]
    async fn test_empty_file_handling() {
        let meta = FileMetadata {
            file_name: "empty.txt".into(),
            file_size: Some(0),
            content_type: None,
            supports_range: true,
            etag: None,
            last_modified: None,
        };
        let protocol = ProtocolKind::Mock(MockProto::new(meta));
        let storage = StorageKind::memory();
        let mut task = make_task(
            protocol,
            storage,
            DownloadConfig {
                verify_checksum: false,
                ..test_config()
            },
        );

        task.probe().await.unwrap();
        let frags = task.plan().unwrap();
        assert!(frags.is_empty(), "空文件不应产生分片");

        task.execute().await.unwrap();
        assert_eq!(task.state(), DownloadState::Completed);
        assert!(
            (task.progress() - 1.0).abs() < f64::EPSILON,
            "空文件进度应为 1.0"
        );
    }

    #[tokio::test]
    async fn test_empty_file_unknown_size() {
        let meta = FileMetadata {
            file_name: "stream.dat".into(),
            file_size: None, // 未知大小
            content_type: None,
            supports_range: false,
            etag: None,
            last_modified: None,
        };
        let protocol = ProtocolKind::Mock(MockProto::new(meta));
        let storage = StorageKind::memory();
        let mut task = make_task(
            protocol,
            storage,
            DownloadConfig {
                verify_checksum: false,
                ..test_config()
            },
        );

        task.probe().await.unwrap();
        let frags = task.plan().unwrap();
        // 未知大小视为 0,不产生分片
        assert!(frags.is_empty());
    }

    // ------ 补充: 零大小文件进度 -----

    #[test]
    fn test_progress_zero_size_fragments() {
        let protocol = ProtocolKind::Mock(MockProto::new(test_metadata("z.bin", 0)));
        let storage = StorageKind::memory();
        let mut task = make_task(protocol, storage, test_config());

        // 分片 size 为 0 时进度应为 1.0
        task.fragments = vec![FragmentRecord::new(
            FragmentInfo {
                index: 0,
                start: 0,
                end: 0,
                size: 0,
                downloaded: 0,
                hash: None,
            },
            3,
        )];
        assert!((task.progress() - 1.0).abs() < f64::EPSILON);
    }

    // ------ 补充: URL 解析校验 -----

    #[tokio::test]
    async fn test_invalid_url_fails() {
        let config = test_config();
        let result = DownloadTask::new("not a url".into(), config).await;
        assert!(result.is_err(), "非法 URL 应创建失败");
    }

    // ------ 补充: run 失败时状态标记 -----

    #[tokio::test]
    async fn test_run_failure_marks_state() {
        let protocol = ProtocolKind::Mock(MockProto::failing(QfError::Network("断网".into())));
        let storage = StorageKind::memory();
        let mut task = make_task(
            protocol,
            storage,
            DownloadConfig {
                verify_checksum: false,
                ..test_config()
            },
        );

        let result = task.run().await;
        assert!(result.is_err());
        assert_eq!(task.state(), DownloadState::Failed);
    }
}
