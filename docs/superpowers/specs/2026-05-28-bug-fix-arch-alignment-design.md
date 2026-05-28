# QuantumFetch Bug 修复 + 架构对齐设计

> 2026-05-28 审计后设计。聚焦 15 项未解决问题的修复，不涉及新功能。
> 基于 `docx/全面优化计划.md` 阶段二核心项。

## 目标

修复现有 Bug、消除死代码、对齐架构契约，使下载管线从"可运行"变为"可优化"。
不涉及：GPU shader、DHT RPC、MP-QUIC、浏览器扩展、持久化层替换。

## 审计发现的 15 项问题

| # | 问题 | 严重性 | 层 |
|---|------|:------:|:---:|
| 1 | ConnectionPool 非全局单例（AppState 和 Orchestrator 各自创建独立实例） | 高 | L2 |
| 2 | I/O 层 Mutex\<File\> 串行化（无 pread/pwrite） | 高 | L3 |
| 3 | download_range 返回 Bytes（流式 API 已存在但管线未使用） | 高 | L4 |
| 4 | FragmentRecord.data 仍存在（64MB 驻留内存） | 高 | L4 |
| 5 | verify() 每分片分配 vec![0u8; frag_size] | 高 | L4 |
| 6 | TaskInfo.status 仍为 String | 中 | L1 |
| 7 | AppError 已定义但 Tauri 命令仍返回 Result\<T, String\> | 中 | L1 |
| 8 | 两套 AppConfig（qf-core 嵌套版 vs qf-app 扁平版） | 中 | L2 |
| 9 | User-Agent 4 处重复定义 | 中 | L1 |
| 10 | 嵌套锁模式（tasks->handles） | 中 | L1 |
| 11 | O(n) 分片查找 | 中 | L4 |
| 12 | AppState.http_client 死代码 | 中 | L1 |
| 13 | FragmentState/HashAlgorithm 缺 Default | 低 | L1 |
| 14 | PeerScore/PeerInfo 缺 PartialEq | 低 | L1 |
| 15 | AppState.connection_pool 死代码 | 中 | L2 |

## 分层设计

### L1：死代码清除 + 类型对齐（可并行执行）

#### 1a. AppError 激活

- 所有 Tauri 命令返回类型从 `Result<T, String>` 改为 `Result<T, AppError>`
- `AppError` 已有 `#[derive(Serialize)]`，Tauri 自动序列化
- 需添加 `impl From<AppError> for tauri::ipc::InvokeError` 或使用 `tauri::command` 宏的错误处理
- 文件：`crates/qf-app/src/commands.rs`

#### 1b. TaskInfo.status -> DownloadState 枚举

- `TaskInfo.status: String` -> `TaskInfo.status: DownloadState`
- `DownloadProgress.status` 和 `TaskProgress.status` 同步改为 `DownloadState`
- 移除 `status` 模块字符串常量（commands.rs:207-214）
- 所有比较改为枚举 match
- `DownloadState` 添加 `#[serde(rename_all = "lowercase")]`，JSON 输出为 "downloading", "paused" 等，与前端一致
- 前端 types.ts `status: string` -> `status: DownloadState`（联合类型 `'pending' | 'downloading' | 'paused' | 'verifying' | 'completed' | 'failed' | 'cancelled'`）
- 文件：`crates/qf-app/src/commands.rs`、`crates/qf-core/src/types.rs`、`frontend/src/types.ts`、前端组件

#### 1c. User-Agent 统一

- qf-app commands.rs 两处硬编码改为引用 `qf_core::config::USER_AGENT`
- qf-protocol http.rs 的 `format!("QuantumFetch/{}", env!("CARGO_PKG_VERSION"))` 改为引用同一常量
- qf-protocol 需添加对 qf-core 的依赖（如果尚未有）
- 文件：`crates/qf-app/src/commands.rs`、`crates/qf-protocol/src/http.rs`

#### 1d. AppState 死代码移除

- 移除 `AppState.http_client: Arc<reqwest::Client>`（管线不使用）
- 移除 `AppState.connection_pool: Arc<ConnectionPool>`（L2 中重建）
- 移除 `AppState::new()` 中创建 reqwest Client 的代码
- 文件：`crates/qf-app/src/commands.rs`

#### 1e. 补充缺失 trait 实现

- `FragmentState` 添加 `#[default]` on `Pending`
- `HashAlgorithm` 添加 `#[default]` on `Blake3`
- `PeerScore` 添加 `PartialEq`（stability 比较使用阈值 epsilon 或将 f64 包装为 OrderedFloat 等价物）
- `PeerInfo` 添加 `Default`
- 文件：`crates/qf-engine/src/fragment.rs`、`crates/qf-crypto/src/cpu.rs`、`crates/qf-p2sp/src/peer.rs`

#### 1f. 嵌套锁模式修复

- `handles` 改为 `DashMap<String, JoinHandle<()>>` 替代 `Arc<Mutex<HashMap<String, JoinHandle<()>>>>`
- DashMap 消除嵌套锁风险：cancel/delete 操作无需先锁 handles 再锁 tasks
- `cancel_task`：DashMap remove + abort，然后锁 tasks 更新状态（无嵌套）
- `delete_task`：同上
- 文件：`crates/qf-app/src/commands.rs`

### L2：全局单例 + 配置统一（依赖 L1 完成后执行）

#### 2a. ConnectionPool 全局单例

- `AppState` 持有 `Arc<ConnectionPool>`，从 `AppConfig.connection` 构建 `PoolConfig`
- `DownloadTask` 新增 `pool: Arc<ConnectionPool>` 字段
- `DownloadOrchestrator.pool` 从 `ConnectionPool` 改为 `Arc<ConnectionPool>`
- `create_task` 将 `state.connection_pool` 传给 `DownloadTask`
- 文件：`crates/qf-app/src/commands.rs`、`crates/qf-engine/src/downloader.rs`、`crates/qf-engine/src/orchestrator.rs`

#### 2b. 两套 AppConfig 合一

- 移除 `qf-app::commands::AppConfig`（扁平版，131-144行）
- 使用 `qf-core::config::AppConfig`（嵌套版：download, connection, scheduler）
- qf-app 添加 `use qf_core::config::AppConfig`
- `get_config`/`update_config` 直接操作 qf-core 的 `AppConfig`
- 前端 `AppConfig` 类型改为嵌套结构
- 前端 SettingsPanel 适配嵌套配置字段名
- 文件：`crates/qf-app/src/commands.rs`、`frontend/src/types.ts`、`frontend/src/components/SettingsPanel.tsx`

#### 2c. PoolConfig <- ConnectionConfig 自动生成

- `From<&ConnectionConfig> for PoolConfig` 已存在（connection.rs:33-51）
- `AppState::new()` 从 `AppConfig.connection` 生成 `PoolConfig`，不再硬编码
- 文件：`crates/qf-app/src/commands.rs`、`crates/qf-engine/src/connection.rs`

### L3：I/O 层 positioned I/O（与 L1 可并行执行）

#### 3a. TokioFile — positioned I/O

- 移除 `Mutex<tokio::fs::File>`
- Windows：使用标准 `File` + seek+write/pwrite 但改为 `RwLock` 允许并发读
- Linux：使用 `nix::sys::pread/pwrite` 实现真正的定位 I/O
- `write_at(offset, data)` -> 定位写入，无需全局锁
- `read_at(offset, buf)` -> 定位读取
- 条件编译：`#[cfg(target_os = "linux")]` vs `#[cfg(target_os = "windows")]`
- 文件：`crates/qf-io/src/tokio_file.rs`

#### 3b. WinFile — OVERLAPPED I/O

- 移除 `Mutex<File>`
- 使用 `OVERLAPPED{Offset, OffsetHigh}` 结构指定写入位置
- `write_at` 通过 `WriteFile` + OVERLAPPED 指定位置
- `read_at` 通过 `ReadFile` + OVERLAPPED 指定位置
- 不使用 `FILE_FLAG_OVERLAPPED`（需要 IOCP 完整异步框架，超出本次范围）
- 使用同步 OVERLAPPED 调用替代 Mutex+seek，消除串行化
- 文件：`crates/qf-io/src/winio.rs`

#### 3c. AsyncStorage trait 不变

- `AsyncStorage` trait 签名不变（`write_at`/`read_at` 已接收 offset 参数）
- 实现层内部重构，上层代码零改动

### L4：流式下载管线（依赖 L3 完成后执行）

#### 4a. 启用 download_range_stream

- `downloader.rs` 的 `execute_fragmented_download` 从 `download_range` 改为 `download_range_stream`
- 流式读取 chunks，通过 BufferPool 缓冲后写入 Storage
- 每 chunk 大小由 BufferPool 的 chunk_size 决定（1MB）
- 峰值内存从 16x64MB=1GB 降至 16x1MB=16MB
- 文件：`crates/qf-engine/src/downloader.rs`

#### 4b. 移除 FragmentRecord.data

- `FragmentRecord` 移除 `pub data: Option<Bytes>` 字段
- `complete_download()` 不再设置 `self.data = Some(data)`
- `mark_failed()` 不再设置 `self.data = None`
- 数据流经 Storage，不驻留分片记录
- 文件：`crates/qf-engine/src/fragment.rs`

#### 4c. 流式 verify

- `verify()` 不再分配 `vec![0u8; frag_size]`
- 分块读取（1MB chunk）+ 流式哈希更新
- 使用 `blake3::Hasher::update()` 分块计算
- 文件：`crates/qf-engine/src/downloader.rs`

#### 4d. 分片查找 O(1)

- `Vec<FragmentRecord>` 按 index 顺序存储（创建时保证 0,1,2,...）
- `self.fragments[index as usize]` 直接访问替代 `iter().find()`
- 文件：`crates/qf-engine/src/downloader.rs`、`crates/qf-engine/src/orchestrator.rs`

## 执行依赖图

```
L1 (死代码清除 + 类型对齐) ──┐
                              ├──> L2 (全局单例 + 配置统一)
L3 (I/O positioned I/O)   ──┼──> L4 (流式下载管线)
                              │
  L1 和 L3 可并行            │
  L2 依赖 L1                 │
  L4 依赖 L3                 │
```

## 子代理分派策略

| 波次 | 并行任务 | 依赖 |
|:----:|---------|------|
| 1 | L1 全部（6 项）+ L3 全部（3 项）= 9 项任务并行 | 无 |
| 2 | L2 全部（3 项）| L1 完成 |
| 3 | L4 全部（4 项）| L3 完成 |

每项任务由独立子代理执行，完成后主代理验证 clippy + 测试。

## 前端影响

| 改动 | 文件 |
|------|------|
| TaskInfo.status 类型更新 | types.ts, DownloadCard.tsx, DetailPanel.tsx |
| AppConfig 嵌套结构 | types.ts, SettingsPanel.tsx |
| DownloadState 联合类型 | types.ts |

## 风险

| 风险 | 缓解 |
|------|------|
| positioned I/O 跨平台兼容性 | Windows 用 OVERLAPPED 同步调用（非 IOCP），Linux 用 nix pread/pwrite，macOS 回退 RwLock+seek |
| AppConfig 合并导致前端 breaking change | 前端同步更新，保持 camelCase serde 序列化 |
| 流式管线引入新 Bug | 每个 L 层完成后运行完整测试 + clippy |
| FragmentRecord.data 移除影响 complete_download | complete_download 的调用者需检查 data 字段使用情况 |
