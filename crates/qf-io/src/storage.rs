//! 异步存储抽象

use qf_core::QfResult;

pub trait AsyncStorage: Send + Sync {
    fn write_at(
        &self,
        offset: u64,
        data: &[u8],
    ) -> impl std::future::Future<Output = QfResult<usize>> + Send;

    fn read_at(
        &self,
        offset: u64,
        buf: &mut [u8],
    ) -> impl std::future::Future<Output = QfResult<usize>> + Send;

    fn sync(&self) -> impl std::future::Future<Output = QfResult<()>> + Send;

    fn allocate(&self, size: u64) -> impl std::future::Future<Output = QfResult<()>> + Send;

    fn file_size(&self) -> impl std::future::Future<Output = QfResult<u64>> + Send;

    fn close(&self) -> impl std::future::Future<Output = QfResult<()>> + Send;
}
