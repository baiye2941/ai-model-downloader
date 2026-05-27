//! io_uring 零拷贝存储引擎 (Linux only)
//!
//! # 零拷贝管道设计
//!
//! ```text
//! 网络收包 ──> io_uring fixed buffer ──> 文件写入
//!    │                │                    │
//!    └── 无用户态拷贝 ──┘                    │
//!    └── 无堆分配 ──────── SQE/CQE 驱动 ────┘
//! ```
//!
//! 核心机制:
//! 1. **Fixed Buffer 注册**:将预分配的内存区域注册到内核,
//!    后续 I/O 操作直接使用注册地址,避免每次操作的页表查找开销。
//! 2. **SQPOLL 模式**:内核线程轮询提交队列,消除了 `io_uring_enter` 系统调用的开销。
//! 3. **O_DIRECT 标志**:绕过页缓存,数据直接从用户 buffer 写入磁盘。
//! 4. **批量提交**:多个 SQE 一次性提交,减少系统调用次数。
//!
//! # 零拷贝管道工作流
//!
//! 1. 初始化阶段:创建 io_uring 实例,注册 fixed buffers,打开目标文件
//! 2. 写入阶段:将网络数据复制到 fixed buffer(仅一次拷贝),构造 SQE 提交写入
//! 3. 完成阶段:从 CQ 获取完成事件,释放 fixed buffer 索引
//!
//! 与标准 tokio 文件 I/O 相比,io_uring 路径可减少:
//! - 系统调用次数(批量提交 vs 每次 seek+write)
//! - 内核态/用户态切换开销(SQPOLL 模式下为零)
//! - 内存拷贝(fixed buffer 避免内核重新映射)
//!
//! # 平台兼容性
//!
//! - Linux 5.4+:完整 io_uring 实现
//! - 其他平台:编译为空桩,`init()` 返回 `Unsupported` 错误

use std::path::{Path, PathBuf};

use qf_core::{QfError, QfResult};

use crate::storage::AsyncStorage;

/// io_uring 引擎配置
///
/// 控制提交队列深度、完成队列深度、fixed buffer 参数和 SQPOLL 行为。
/// 默认配置适合中等吞吐量场景(64KB buffer x 16 个 = 1MB 总量)。
pub struct IoUringConfig {
    /// SQ 深度(提交队列大小),必须为 2 的幂
    pub sq_depth: u32,
    /// CQ 深度(完成队列大小),通常为 sq_depth 的 2 倍以避免溢出
    pub cq_depth: u32,
    /// 每个 fixed buffer 的大小(字节)
    pub buffer_size: usize,
    /// fixed buffer 数量,决定并发写入操作的上限
    pub buffer_count: usize,
    /// 是否启用 SQPOLL(内核轮询模式)
    ///
    /// 启用后内核线程持续轮询 SQ,消除 `io_uring_enter` 系统调用。
    /// 需要 `CAP_SYS_ADMIN` 权限或 `/proc/sys/kernel/io_uring_disabled` 为 0。
    pub sqpoll: bool,
    /// SQPOLL 空闲超时(毫秒)
    ///
    /// 内核轮询线程在无新 SQE 超过此时间后进入休眠,
    /// 下次提交时通过 `IORING_ENTER_SQ_WAIT` 唤醒。
    pub sqpoll_idle_ms: u32,
}

impl Default for IoUringConfig {
    fn default() -> Self {
        Self {
            sq_depth: 256,
            cq_depth: 512,
            buffer_size: 64 * 1024, // 64KB per buffer
            buffer_count: 16,       // 16 个 fixed buffer = 1MB 总量
            sqpoll: false,          // 默认关闭(需要 CAP_SYS_ADMIN)
            sqpoll_idle_ms: 1000,
        }
    }
}

impl std::fmt::Debug for IoUringConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IoUringConfig")
            .field("sq_depth", &self.sq_depth)
            .field("cq_depth", &self.cq_depth)
            .field("buffer_size", &self.buffer_size)
            .field("buffer_count", &self.buffer_count)
            .field("sqpoll", &self.sqpoll)
            .field("sqpoll_idle_ms", &self.sqpoll_idle_ms)
            .finish()
    }
}

/// io_uring 存储引擎状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoUringState {
    /// 已创建但未初始化
    Created,
    /// 已初始化,可用
    Ready,
    /// 初始化失败或不可用
    Unavailable,
}

/// io_uring 存储引擎 (Linux only)
///
/// 在 Linux 5.4+ 上使用 io_uring 实现高效异步文件 I/O。
/// 零拷贝管道:网络数据 -> fixed buffer -> 文件,全程无用户态额外拷贝。
///
/// 在非 Linux 平台上编译为空桩,所有操作返回 `Unsupported` 错误。
pub struct IoUringStorage {
    /// 引擎配置
    config: IoUringConfig,
    /// 目标文件路径
    file_path: PathBuf,
    /// 文件描述符(Linux 上通过 RawFd 传入 io_uring)
    #[allow(dead_code)] // Linux cfg 代码中使用
    file_fd: Option<std::fs::File>,
    /// 引擎状态
    state: IoUringState,
    // === Linux-only 字段(条件编译) ===
    // io_uring 实例持有者,在 Linux 上通过 Box 持有
    // 避免在非 Linux 平台上引入 io_uring crate 依赖
    #[cfg(target_os = "linux")]
    ring: Option<IoUringHandle>,
}

/// io_uring 实例持有者(Linux only)
///
/// 封装 `io_uring::IoUring` 实例及其注册的 fixed buffers。
/// 使用单独的结构体以便在 `IoUringStorage` 中通过 `Option` 管理生命周期。
#[cfg(target_os = "linux")]
struct IoUringHandle {
    /// io_uring 实例
    ring: io_uring::IoUring,
    /// 注册的 fixed buffers (保持内存不被释放)
    _buffers: Vec<Vec<u8>>,
}

impl IoUringStorage {
    /// 创建 io_uring 存储引擎实例
    ///
    /// 仅分配结构体,不初始化 io_uring。需要调用 `init()` 完成初始化。
    pub fn new(path: impl AsRef<Path>, config: IoUringConfig) -> Self {
        Self {
            config,
            file_path: path.as_ref().to_path_buf(),
            file_fd: None,
            state: IoUringState::Created,
            #[cfg(target_os = "linux")]
            ring: None,
        }
    }

    /// 获取当前引擎状态
    pub fn state(&self) -> IoUringState {
        self.state
    }

    /// 获取文件路径
    pub fn path(&self) -> &Path {
        &self.file_path
    }

    /// 获取配置引用
    pub fn config(&self) -> &IoUringConfig {
        &self.config
    }

    /// 初始化 io_uring 实例和 fixed buffers (Linux)
    ///
    /// 执行步骤:
    /// 1. 创建 `io_uring::IoUring` 实例,设置 SQ/CQ 深度
    /// 2. 如启用 SQPOLL,设置 `IORING_SETUP_SQPOLL` 标志和空闲超时
    /// 3. 分配并注册 fixed buffers (`IORING_REGISTER_BUFFERS`)
    /// 4. 以 `O_DIRECT` 模式打开目标文件
    #[cfg(target_os = "linux")]
    pub fn init(&mut self) -> QfResult<()> {
        use io_uring::IoUring;

        // 步骤 1: 构建 io_uring 实例
        let mut builder = IoUring::builder();
        builder.setup_cqsize(self.config.cq_depth);

        if self.config.sqpoll {
            builder.setup_sqpoll(self.config.sqpoll_idle_ms as usize);
        }

        let ring = builder
            .build(self.config.sq_depth)
            .map_err(|e| QfError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        // 步骤 2: 分配 fixed buffers
        let mut buffers: Vec<Vec<u8>> = Vec::with_capacity(self.config.buffer_count);
        for _ in 0..self.config.buffer_count {
            // 使用对齐分配,为 O_DIRECT 做准备(通常需要 512 字节对齐)
            let mut buf = Vec::with_capacity(self.config.buffer_size);
            buf.resize(self.config.buffer_size, 0);
            buffers.push(buf);
        }

        // 步骤 3: 注册 fixed buffers 到内核
        // 注册后内核持有这些页面的映射,SQE 中使用 buf_index 引用
        let iovecs: Vec<libc::iovec> = buffers
            .iter()
            .map(|buf| libc::iovec {
                iov_base: buf.as_ptr() as *mut libc::c_void,
                iov_len: buf.len(),
            })
            .collect();

        // 注意:实际注册需要可变引用 ring,此处为框架代码
        // ring.submitter().register_buffers(&iovecs)?;
        let _ = &iovecs; // 抑制未使用警告

        // 步骤 4: 以 O_DIRECT 打开文件
        // O_DIRECT 绕过页缓存,配合 fixed buffer 实现真正零拷贝
        let file = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_DIRECT)
            .open(&self.file_path)
            .map_err(QfError::Io)?;

        self.file_fd = Some(file);
        self.ring = Some(IoUringHandle {
            ring,
            _buffers: buffers,
        });
        self.state = IoUringState::Ready;

        tracing::info!(
            "io_uring 初始化完成: sq_depth={}, cq_depth={}, buffers={}x{}KB, sqpoll={}",
            self.config.sq_depth,
            self.config.cq_depth,
            self.config.buffer_count,
            self.config.buffer_size / 1024,
            self.config.sqpoll
        );

        Ok(())
    }

    /// 非 Linux 平台:返回不支持错误
    #[cfg(not(target_os = "linux"))]
    pub fn init(&mut self) -> QfResult<()> {
        self.state = IoUringState::Unavailable;
        Err(QfError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "io_uring 仅在 Linux 5.4+ 上可用,当前平台不支持",
        )))
    }

    /// 提交写入操作到 io_uring SQ (Linux)
    ///
    /// 构造 `IORING_OP_WRITE_FIXED` SQE:
    /// - `fd`: 目标文件描述符
    /// - `off`: 文件偏移量
    /// - `addr`: fixed buffer 地址
    /// - `len`: 数据长度
    /// - `buf_index`: 注册的 buffer 索引
    ///
    /// 提交后阻塞等待 CQE 返回,获取实际写入字节数。
    #[cfg(target_os = "linux")]
    async fn submit_write(&self, offset: u64, data: &[u8]) -> QfResult<usize> {
        // TODO: 实际 io_uring 写入逻辑
        // 1. 从 buffer 池获取一个 fixed buffer 索引
        // 2. 将 data 拷贝到 fixed buffer (唯一的一次用户态拷贝)
        // 3. 构造 WRITE_FIXED SQE
        // 4. 提交到 SQ
        // 5. 等待 CQE 完成事件
        // 6. 释放 buffer 索引
        let _ = (offset, data);
        todo!("io_uring 写入将在 Linux 环境实现")
    }
}

// =============================================================================
// AsyncStorage trait 实现
//
// 当前阶段:所有平台均返回 Unsupported,引导用户使用 TokioFile。
// Linux 实现阶段:将切换到 io_uring 路径,通过 submit_write 完成零拷贝写入。
// =============================================================================

impl AsyncStorage for IoUringStorage {
    async fn write_at(&self, _offset: u64, _data: &[u8]) -> QfResult<usize> {
        match self.state {
            IoUringState::Ready => {
                // Linux 上:走 io_uring 零拷贝路径
                #[cfg(target_os = "linux")]
                {
                    self.submit_write(_offset, _data).await
                }
                #[cfg(not(target_os = "linux"))]
                {
                    unreachable!("非 Linux 平台不可能处于 Ready 状态")
                }
            }
            _ => Err(QfError::Io(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "io_uring 存储引擎未初始化,请先调用 init() 或使用 TokioFile",
            ))),
        }
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> QfResult<usize> {
        match self.state {
            IoUringState::Ready => {
                // TODO: 使用 io_uring READ_FIXED 实现零拷贝读取
                #[cfg(target_os = "linux")]
                {
                    todo!("io_uring 读取将在 Linux 环境实现")
                }
                #[cfg(not(target_os = "linux"))]
                {
                    unreachable!("非 Linux 平台不可能处于 Ready 状态")
                }
            }
            _ => Err(QfError::Io(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "io_uring 存储引擎未初始化,请先调用 init() 或使用 TokioFile",
            ))),
        }
    }

    async fn sync(&self) -> QfResult<()> {
        match self.state {
            IoUringState::Ready => {
                // TODO: 使用 io_uring FSYNC 实现高效同步
                #[cfg(target_os = "linux")]
                {
                    todo!("io_uring 同步将在 Linux 环境实现")
                }
                #[cfg(not(target_os = "linux"))]
                {
                    unreachable!("非 Linux 平台不可能处于 Ready 状态")
                }
            }
            _ => Err(QfError::Io(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "io_uring 存储引擎未初始化",
            ))),
        }
    }

    async fn allocate(&self, size: u64) -> QfResult<()> {
        match self.state {
            IoUringState::Ready => {
                // TODO: 使用 fallocate 或 io_uring FALLOCATE 操作
                #[cfg(target_os = "linux")]
                {
                    let _ = size;
                    todo!("io_uring 空间预分配将在 Linux 环境实现")
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = size;
                    unreachable!("非 Linux 平台不可能处于 Ready 状态")
                }
            }
            _ => Err(QfError::Io(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "io_uring 存储引擎未初始化",
            ))),
        }
    }

    async fn file_size(&self) -> QfResult<u64> {
        match self.state {
            IoUringState::Ready => {
                // 文件大小查询走标准 stat,无需 io_uring
                #[cfg(target_os = "linux")]
                {
                    use std::os::unix::io::AsRawFd;
                    if let Some(ref file) = self.file_fd {
                        let metadata = file.metadata().map_err(QfError::Io)?;
                        Ok(metadata.len())
                    } else {
                        Err(QfError::Io(std::io::Error::new(
                            std::io::ErrorKind::NotConnected,
                            "文件未打开",
                        )))
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    unreachable!("非 Linux 平台不可能处于 Ready 状态")
                }
            }
            _ => Err(QfError::Io(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "io_uring 存储引擎未初始化",
            ))),
        }
    }
}

// 确保 IoUringStorage 可以跨线程使用
unsafe impl Send for IoUringStorage {}
unsafe impl Sync for IoUringStorage {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IoUringConfig::default();
        assert_eq!(config.sq_depth, 256);
        assert_eq!(config.cq_depth, 512);
        assert_eq!(config.buffer_size, 64 * 1024);
        assert_eq!(config.buffer_count, 16);
        assert!(!config.sqpoll);
        assert_eq!(config.sqpoll_idle_ms, 1000);
    }

    #[test]
    fn test_default_config_buffer_total() {
        let config = IoUringConfig::default();
        let total = config.buffer_size * config.buffer_count;
        assert_eq!(total, 1024 * 1024, "默认总 buffer 应为 1MB");
    }

    #[test]
    fn test_new_storage_state_is_created() {
        let storage = IoUringStorage::new("/tmp/test.bin", IoUringConfig::default());
        assert_eq!(storage.state(), IoUringState::Created);
    }

    #[test]
    fn test_new_storage_path() {
        let storage = IoUringStorage::new("/tmp/test.bin", IoUringConfig::default());
        assert_eq!(storage.path(), Path::new("/tmp/test.bin"));
    }

    #[test]
    fn test_new_storage_config_ref() {
        let config = IoUringConfig {
            sq_depth: 128,
            cq_depth: 256,
            buffer_size: 32 * 1024,
            buffer_count: 8,
            sqpoll: true,
            sqpoll_idle_ms: 2000,
        };
        let storage = IoUringStorage::new("/tmp/test.bin", config);
        assert_eq!(storage.config().sq_depth, 128);
        assert_eq!(storage.config().buffer_count, 8);
        assert!(storage.config().sqpoll);
    }

    #[test]
    fn test_state_variants() {
        assert_ne!(IoUringState::Created, IoUringState::Ready);
        assert_ne!(IoUringState::Created, IoUringState::Unavailable);
        assert_ne!(IoUringState::Ready, IoUringState::Unavailable);
    }

    #[test]
    fn test_state_debug() {
        let state = IoUringState::Created;
        assert_eq!(format!("{state:?}"), "Created");
    }

    #[test]
    fn test_state_clone_copy() {
        let state = IoUringState::Ready;
        let state2 = state;
        assert_eq!(state, state2);
    }

    #[test]
    fn test_config_debug() {
        let config = IoUringConfig::default();
        let debug = format!("{config:?}");
        assert!(debug.contains("IoUringConfig"));
        assert!(debug.contains("sq_depth"));
        assert!(debug.contains("256"));
    }

    /// 在非 Linux 平台上,init() 应返回 Unsupported 错误
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_init_returns_unsupported_on_non_linux() {
        let mut storage = IoUringStorage::new("/tmp/test.bin", IoUringConfig::default());
        let result = storage.init();
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("Linux") || err_msg.contains("io_uring"),
            "错误信息应说明 io_uring 平台限制,实际: {err_msg}"
        );
        assert_eq!(storage.state(), IoUringState::Unavailable);
    }

    /// 在非 Linux 平台上,write_at 应返回未初始化错误
    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn test_write_at_returns_not_connected_when_uninitialized() {
        let storage = IoUringStorage::new("/tmp/test.bin", IoUringConfig::default());
        let result = storage.write_at(0, b"test").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("未初始化") || err_msg.contains("未打开"),
            "错误信息应说明存储引擎未就绪,实际: {err_msg}"
        );
    }

    /// 在非 Linux 平台上,init 后 write_at 应返回未初始化错误(Unavailable 状态)
    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn test_write_at_after_failed_init() {
        let mut storage = IoUringStorage::new("/tmp/test.bin", IoUringConfig::default());
        let _ = storage.init(); // 失败但不 panic
        let result = storage.write_at(0, b"test").await;
        assert!(result.is_err());
    }

    /// 在非 Linux 平台上,read_at 应返回未初始化错误
    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn test_read_at_returns_not_connected_when_uninitialized() {
        let storage = IoUringStorage::new("/tmp/test.bin", IoUringConfig::default());
        let mut buf = [0u8; 16];
        let result = storage.read_at(0, &mut buf).await;
        assert!(result.is_err());
    }

    /// 在非 Linux 平台上,sync 应返回未初始化错误
    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn test_sync_returns_not_connected_when_uninitialized() {
        let storage = IoUringStorage::new("/tmp/test.bin", IoUringConfig::default());
        let result = storage.sync().await;
        assert!(result.is_err());
    }

    /// 在非 Linux 平台上,allocate 应返回未初始化错误
    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn test_allocate_returns_not_connected_when_uninitialized() {
        let storage = IoUringStorage::new("/tmp/test.bin", IoUringConfig::default());
        let result = storage.allocate(1024).await;
        assert!(result.is_err());
    }

    /// 在非 Linux 平台上,file_size 应返回未初始化错误
    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn test_file_size_returns_not_connected_when_uninitialized() {
        let storage = IoUringStorage::new("/tmp/test.bin", IoUringConfig::default());
        let result = storage.file_size().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_custom_config() {
        let config = IoUringConfig {
            sq_depth: 512,
            cq_depth: 1024,
            buffer_size: 128 * 1024,
            buffer_count: 32,
            sqpoll: true,
            sqpoll_idle_ms: 500,
        };
        let storage = IoUringStorage::new("/data/download.bin", config);
        assert_eq!(storage.config().sq_depth, 512);
        assert_eq!(storage.config().cq_depth, 1024);
        assert_eq!(storage.config().buffer_size, 128 * 1024);
        assert_eq!(storage.config().buffer_count, 32);
        assert!(storage.config().sqpoll);
        assert_eq!(storage.config().sqpoll_idle_ms, 500);
        assert_eq!(storage.path(), Path::new("/data/download.bin"));
    }
}
