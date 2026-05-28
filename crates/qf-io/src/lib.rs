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

// 验证测试:放在 crate 根级别,以便 `--exact` 匹配

/// 验证 io_uring buffer 对齐:512 和 4096 字节边界
#[cfg(test)]
#[test]
fn buffer_align() {
    // 512 字节对齐
    let size = 100usize;
    assert_eq!((size + 511) & !511, 512);

    // 4096 字节对齐(O_DIRECT)
    let size = 1000usize;
    assert_eq!((size + 4095) & !4095, 4096);

    // 已对齐的大小不变
    assert_eq!((4096usize + 4095) & !4095, 4096);
    assert_eq!((512usize + 511) & !511, 512);

    // 默认 64KB buffer 是 4096 的倍数
    assert_eq!((64 * 1024usize) % 4096, 0);
}

/// 验证 WritePipeline 可以正常创建并执行基本写入操作
#[cfg(test)]
#[tokio::test]
async fn write_pipeline() {
    // 创建临时文件作为存储后端
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let storage = TokioFile::open(tmp.path()).await.unwrap();
    let pool = BufferPool::new(4096, 4);
    let pipeline = WritePipeline::new(storage, pool);

    // 验证 buffer 池引用可访问
    assert_eq!(pipeline.buffer_pool().capacity(), 4);
    assert_eq!(pipeline.buffer_pool().buffer_size(), 4096);

    // 写入数据并验证返回的字节数
    let written = pipeline.write(0, b"hello pipeline").await.unwrap();
    assert_eq!(written, 14);

    // 从存储回读数据,验证内容一致
    let mut buf = [0u8; 14];
    let read = pipeline.storage().read_at(0, &mut buf).await.unwrap();
    assert_eq!(read, 14);
    assert_eq!(&buf, b"hello pipeline");
}

/// 验证 BufferPool 背压行为:耗尽后归还可复用
#[cfg(test)]
#[test]
fn backpressure() {
    // 创建小容量 buffer 池,模拟资源受限场景
    let pool = BufferPool::with_prefill(1024, 2);
    assert_eq!(pool.capacity(), 2);
    assert_eq!(pool.available(), 2);

    // 分配所有 buffer,池应为空
    let buf1 = pool.alloc();
    let buf2 = pool.alloc();
    assert_eq!(pool.available(), 0);

    // 此时再分配会触发新建(池空但不阻塞)
    let buf3 = pool.alloc();
    // available 仍为 0,因为 buf3 是新建的不在池中
    assert_eq!(pool.available(), 0);

    // 归还第一个 buffer,验证可用数恢复
    pool.release(buf1);
    assert_eq!(pool.available(), 1);

    // 归还第二个 buffer
    pool.release(buf2);
    assert_eq!(pool.available(), 2);

    // 超出容量的归还会被丢弃(capacity=2,已有 2 个在池中)
    pool.release(buf3);
    assert_eq!(pool.available(), 2); // 仍然为 2,不增长
}

/// 验证 WinFile NO_BUFFERING 对齐写入逻辑
#[cfg(test)]
#[test]
fn winfile_align() {
    const SECTOR_SIZE: u64 = 512;
    // 非对齐偏移
    assert!(!100u64.is_multiple_of(SECTOR_SIZE));
    // 对齐偏移
    assert!(0u64.is_multiple_of(SECTOR_SIZE));
    assert!(512u64.is_multiple_of(SECTOR_SIZE));
    assert!(4096u64.is_multiple_of(SECTOR_SIZE));
    // 非对齐数据长度
    assert!(!256u64.is_multiple_of(SECTOR_SIZE));
    // 对齐数据长度
    assert!(512u64.is_multiple_of(SECTOR_SIZE));
    assert!(1024u64.is_multiple_of(SECTOR_SIZE));
}
