//! QuantumFetch I/O 层:零拷贝存储引擎
//!
//! 提供跨平台的异步文件 I/O:
//! - Linux:io_uring 零拷贝管道
//! - Windows:WinFile 优化(NO_BUFFERING + SEQUENTIAL_SCAN)
//! - macOS:tokio 标准异步文件 I/O
//! - BufferPool 管理与 buffer 复用
//! - 零拷贝写入管道(含批量写入)

pub mod buffer;
pub mod iouring;
pub mod pipeline;
pub mod storage;
pub mod tokio_file;
pub mod winio;

pub use buffer::{BufferPool, BufferPoolStats};
pub use iouring::{IoUringConfig, IoUringState, IoUringStorage};
pub use pipeline::WritePipeline;
pub use storage::AsyncStorage;
pub use tokio_file::TokioFile;
pub use winio::WinFile;
