//! Buffer 池管理

use bytes::BytesMut;

/// 预分配的 buffer 池,减少运行时堆分配
pub struct BufferPool {
    buffer_size: usize,
    capacity: usize,
}

impl BufferPool {
    /// 创建新的 buffer 池
    pub fn new(buffer_size: usize, capacity: usize) -> Self {
        Self {
            buffer_size,
            capacity,
        }
    }

    /// 获取 buffer 大小
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    /// 获取池容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 分配一个 buffer
    pub fn alloc(&self) -> BytesMut {
        BytesMut::with_capacity(self.buffer_size)
    }
}
