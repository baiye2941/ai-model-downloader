# QuantumFetch 阶段一至四优化设计规格

> 基于 2026-05-28 项目审计，聚焦激活真实下载管线和架构重构。

## 一、阶段一：Bug 修复（已完成 70%，剩余项在阶段二自然消解）

### 已修复

- Bug #2: VerifierKind::clone() → derive(Clone)
- Bug #4: unsafe impl Send/Sync 添加安全不变量文档
- Bug #6: O_DIRECT buffer 4096 字节对齐（aligned_alloc）
- Bug #7: WinFile NO_BUFFERING 运行时对齐校验
- Bug #13: new_insecure() 加 #[cfg(test)] 门控
- Bug #15: CSP 启用严格策略
- Bug #18: 信号量 acquire_owned 优雅错误处理

### 在阶段二自然消解

- Bug #1: any_fragment_failed 永远为 false → 真实管线中 JoinHandle 返回 Result，失败时设置标志
- Bug #11: 模拟下载 → 替换为 DownloadTask::run()
- Bug #16: 前端 2s 轮询 → watch channel 替代

### 需单独修复

- Bug #14: innerHTML XSS → 前端改用 Tauri listen() 推送数据，框架迁移在阶段七彻底解决
- Bug #17: expect("元数据已填充") → 改为 ok_or_else(QfError::...)

---

## 二、阶段二：激活真实下载管线

> 核心原则：让 qf-engine/protocol/io/crypto 从死代码变为活跃代码。

### 2.1 替换 task_fn → DownloadTask::run()

**删除**：commands.rs 中 task_fn 的模拟循环（约 130 行），包括 sleep(2ms) + simulated bytes。

**新增**：create_task 后台 tokio task 调用 `DownloadTask::new(url, config).run()`。run() 内部执行 probe→plan→prepare_storage→execute→verify。

### 2.2 进度通信：watch channel

**新增类型**（qf-core/src/types.rs）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    pub downloaded: u64,
    pub speed: u64,
    pub progress: f64,
    pub fragments_done: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadStateChange {
    pub task_id: String,
    pub new_state: DownloadState,
}
```

**AppState 拆分**：

```rust
pub struct AppState {
    pub tasks: Arc<Mutex<HashMap<String, TaskInfo>>>,       // 保留，用于查询
    pub config: Arc<Mutex<AppConfig>>,                        // 保留
    pub handles: DashMap<String, JoinHandle<()>>,              // 改为 DashMap
    pub connection_pool: Arc<ConnectionPool>,                  // 全局单例
    pub http_client: Arc<HttpClient>,                          // 全局共享
    pub progress_tx: watch::Sender<HashMap<String, TaskProgress>>,
    pub progress_rx: watch::Receiver<HashMap<String, TaskProgress>>,
    pub state_tx: watch::Sender<HashMap<String, DownloadStateChange>>,
    pub state_rx: watch::Receiver<HashMap<String, DownloadStateChange>>,
    pub pause_tx: watch::Sender<HashMap<String, bool>>,        // true=暂停
    pub pause_rx: watch::Receiver<HashMap<String, bool>>,
    pub active_permits: Arc<AtomicU32>,
    pub sniffer: Arc<Mutex<Vec<SnifferResource>>>,
    pub sniffer_filters: Arc<Mutex<Vec<String>>>,
    pub cancel_tokens: DashMap<String, CancellationToken>,     // 取消信号
}
```

**DownloadTask 签名变更**：

```rust
pub struct DownloadTask {
    url: String,
    config: DownloadConfig,
    protocol: ProtocolKind,
    storage: StorageKind,
    verifier: VerifierKind,
    orchestrator: DownloadOrchestrator,
    // 新增
    progress_tx: watch::Sender<HashMap<String, TaskProgress>>,
    state_tx: watch::Sender<HashMap<String, DownloadStateChange>>,
    cancel_token: CancellationToken,
    pause_rx: watch::Receiver<HashMap<String, bool>>,
}
```

**进度推送**：分片完成时 `progress_tx.send()`，状态转换时 `state_tx.send()`。前端 Tauri 命令 subscribe_progress() 返回 watch::Receiver，changed() 驱动更新（替代 2s 轮询）。

### 2.3 暂停/取消：CancellationToken + watch 暂停标志

- create_task 传入 CancellationToken 和 watch::Receiver<bool>（true=暂停）
- DownloadTask 内部：select! 监听 token.is_cancelled() 和暂停 changed()
- 暂停时分片任务阻塞等待 changed()，恢复后立即继续
- 取消时所有分片任务退出
- 删除 5 分钟暂停超时限制

### 2.4 全局连接池：AppState 持有 Arc<ConnectionPool>

- AppState 新增 connection_pool: Arc<ConnectionPool>
- create_task clone Arc 传给 DownloadTask
- 删除每次新建 DownloadOrchestrator 的 PoolConfig 构造

### 2.5 全局 HttpClient：AppState 持有 Arc<HttpClient>

- AppState 新增 http_client: Arc<HttpClient>
- create_task clone Arc 传给 DownloadTask
- 删除 probe_metadata 函数（约 50 行），由 DownloadTask::probe() 完成

### 2.6 Bug #17 修复

- `downloader.rs:323` 的 `expect("元数据已填充")` 改为 `.ok_or_else(|| QfError::Config("元数据未填充"))`

---

## 三、阶段三：架构重构 + 并发优化 + 配置统一

### 3.1 配置体系统一

**删除**：qf-app::commands::AppConfig

**新增**（qf-core/src/config.rs）：

```rust
pub struct AppConfig {
    pub download_dir: String,
    pub max_concurrent_tasks: u32,
    pub download: DownloadConfig,
    pub connection: ConnectionConfig,
    pub scheduler: SchedulerConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            download_dir: dirs()
                .map(|p| p.join("Downloads").to_string_lossy().to_string())
                .unwrap_or_else(|| ".".to_string()),
            max_concurrent_tasks: 5,
            download: DownloadConfig::default(),
            connection: ConnectionConfig::default(),
            scheduler: SchedulerConfig::default(),
        }
    }
}
```

- PoolConfig 从 ConnectionConfig 生成（From trait），不再独立定义
- 所有 crate 从 qf-core::config 读取配置

### 3.2 配置热更新：watch channel

- AppState 持有 `config_tx: watch::Sender<Arc<AppConfig>>`
- DownloadTask 持有 `config_rx: watch::Receiver<Arc<AppConfig>>`
- 配置变更时 send 新配置，任务在下个分片开始时 changed() 读取

### 3.3 消除重复定义

| 重复项 | 解决方案 |
|--------|---------|
| SnifferResource 两套定义 | qf-app 删除自己的 SnifferResource，re-export qf_sniffer::capture::SnifferResource |
| PoolConfig vs ConnectionConfig | PoolConfig 从 ConnectionConfig 生成（From trait） |
| dirs() 函数 | 保留 qf-core 一份，qf-app re-export |
| user_agent 重复 | 提取为 qf-core::USER_AGENT 常量 |

### 3.4 TaskInfo.status 改为 DownloadState 枚举

- TaskInfo.status 类型从 String 改为 DownloadState
- 删除 commands.rs 的 mod status 常量模块
- #[serde(rename_all = "lowercase")] 保持 JSON 兼容

### 3.5 错误体系增强

- QfError::Other(String) → QfError::Other(Box<dyn Error + Send + Sync>)
- Tauri 命令返回 Result<T, AppError>（已定义），替代 Result<T, String>

### 3.6 嵌套锁顺序规范 + DashMap

- handles 改为 DashMap<String, JoinHandle<()>
- cancel_tokens 改为 DashMap<String, CancellationToken>
- 全局规范：handles → tasks，禁止反向锁获取

### 3.7 ConnectionPool host_semaphores 改为 DashMap

- 消除 Mutex<HashMap> 锁竞争

### 3.8 BufferPool 改为 crossbeam::ArrayQueue

- 消除 std::sync::Mutex<VecDeque> 阻塞和毒化问题

### 3.9 ProtocolKind/StorageKind 改为 Arc<dyn Trait>

**Protocol trait 变更**：

```rust
pub trait Protocol: Send + Sync {
    fn probe(&self, url: &str) -> Pin<Box<dyn Future<Output = QfResult<FileMetadata>> + Send>>;
    fn download_range(&self, url: &str, start: u64, end: u64)
        -> Pin<Box<dyn Future<Output = QfResult<Bytes>> + Send>>;
    fn download_range_stream(&self, url: &str, start: u64, end: u64)
        -> Pin<Box<dyn Future<Output = QfResult<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>>> + Send>>;
}
```

- 移除 RPITIT，改为 Pin<Box<dyn Future>>，使其 object-safe
- DownloadTask.protocol: Arc<dyn Protocol>
- 新增协议变体只需实现 trait，无需改枚举

**Storage 统一**：

- 删除 qf-core::Storage trait
- 全部使用 qf-io::AsyncStorage
- qf-core 仅 re-export qf_io::storage::AsyncStorage
- DownloadTask.storage: Arc<dyn AsyncStorage>

### 3.10 FragmentRecord.data 移除

- 删除 FragmentRecord.data: Option<Bytes> 字段
- 分片数据流经 BufferPool → Storage，不应驻留在记录中

---

## 四、阶段四：Storage 对齐契约 + io_uring + GPU shader

### 4.1 AsyncStorage write_at_aligned

```rust
fn write_at_aligned(&self, offset: u64, data: &[u8], alignment: u64) -> QfResult<usize>;
```

- WinFile NO_BUFFERING 模式：不足对齐时自动 pad + tail trimming
- TokioFile：忽略 alignment 参数（常规 I/O 无对齐要求）
- IoUringStorage：使用 fixed buffer 确保对齐

### 4.2 io_uring 4 个 todo!() 操作

| 操作 | 实现 |
|------|------|
| submit_write | 构建 Write64 SQE，提交到提交队列 |
| read_at | 构建 ReadFixed SQE，使用预注册 fixed buffer |
| sync | 构建 Fsync SQE |
| allocate | 使用 fallocate 或 IoUring Fallocate 操作 |

**优雅降级**：Linux 先尝试 io_uring，失败回退 TokioFile。通过 feature flag `io_uring` 控制（默认 Linux 开启）。

### 4.3 GPU blake3 WGSL compute shader

- 实现 blake3 G-function 7-round 压缩函数的 WGSL compute shader
- GpuVerifier 单例复用 Device/Queue/Pipeline（OnceLock 或 lazy 初始化）
- 64MB 阈值改为可配置
- 完成后移除 CPU 回退路径（GPU 失败时降级到 CpuVerifier，而非同一函数内回退）

### 4.4 WinFile::open_optimized 活化

- StorageKind 选择逻辑：Windows + 大文件 → WinFile（NO_BUFFERING），其余 → TokioFile
- 删除 open_optimized 的死代码标记

### 4.5 WritePipeline 集成 BufferPool

- 从 BufferPool 获取 buffer
- 网络数据读入 pool buffer
- 提交 buffer 给存储层写入
- 写入完成后释放 buffer 回 pool
- 相邻 segment 合并为一次 write_at 调用

---

## 五、实施顺序

按照依赖关系排列：

1. **阶段一剩余**：Bug #17 expect→ok_or_else
2. **阶段二核心**：DownloadTask 签名变更 → AppState 重构 → 删除 task_fn/probe_metadata → 激活 run()
3. **阶段三基础**：AppConfig 合一 → 消除重复定义 → TaskInfo.status→DownloadState → 错误体系
4. **阶段三并发**：DashMap handles/cancel_tokens → BufferPool ArrayQueue → ConnectionPool DashMap
5. **阶段三 trait**：Protocol object-safe → Storage 统一 → FragmentRecord.data 移除
6. **阶段四 I/O**：write_at_aligned → io_uring → WritePipeline 集成
7. **阶段四 GPU**：WGSL blake3 shader → GpuVerifier 单例

---

## 六、不在本规格范围内

以下项推迟到后续迭代：

- 阶段五：反压/度量/事件系统/跨层 tracing
- 阶段六：持久化层 qf-store 集成 + 断点续传
- 阶段七：前端 Solid.js + TypeScript 迁移
- 阶段八：Kademlia DHT RPC + P2SP 网络通信 + 浏览器嗅探
- 阶段九：MP-QUIC + HTTP/3
- 阶段十：测试补全 + Kernel Bypass