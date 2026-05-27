//! QuantumFetch I/O 层:零拷贝存储引擎
//!
//! 提供跨平台的异步文件 I/O:
//! - Linux:io_uring 零拷贝管道
//! - Windows/macOS:tokio 标准异步文件 I/O
//! - BufferPool 管理与 fixed buffer 复用

pub mod buffer;
pub mod storage;

pub use buffer::BufferPool;
pub use storage::AsyncStorage;
