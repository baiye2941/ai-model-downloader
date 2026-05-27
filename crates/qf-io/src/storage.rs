//! 异步存储抽象

use qf_core::QfResult;

/// 异步存储 trait
pub trait AsyncStorage: Send + Sync {
    /// 异步写入数据到指定偏移位置
    fn write_at(
        &self,
        offset: u64,
        data: &[u8],
    ) -> impl std::future::Future<Output = QfResult<usize>> + Send;

    /// 异步从指定偏移读取数据
    fn read_at(
        &self,
        offset: u64,
        buf: &mut [u8],
    ) -> impl std::future::Future<Output = QfResult<usize>> + Send;

    /// 同步数据到磁盘
    fn sync(&self) -> impl std::future::Future<Output = QfResult<()>> + Send;

    /// 预分配文件空间
    fn allocate(&self, size: u64) -> impl std::future::Future<Output = QfResult<()>> + Send;

    /// 获取文件大小
    fn file_size(&self) -> impl std::future::Future<Output = QfResult<u64>> + Send;
}
