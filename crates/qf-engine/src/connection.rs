//! 连接池管理
//!
//! 每个主机维护独立连接池,支持连接复用和并发控制。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::sync::Semaphore;

/// 连接池配置
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// 单主机最大连接数
    pub max_per_host: u32,
    /// 全局最大连接数
    pub max_global: u32,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_per_host: 16,
            max_global: 256,
        }
    }
}

/// 全局连接池管理器
pub struct ConnectionPool {
    config: PoolConfig,
    global_semaphore: Arc<Semaphore>,
    active_count: AtomicU32,
    host_semaphores: tokio::sync::Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl ConnectionPool {
    /// 创建新的连接池
    pub fn new(config: PoolConfig) -> Self {
        Self {
            global_semaphore: Arc::new(Semaphore::new(config.max_global as usize)),
            config,
            active_count: AtomicU32::new(0),
            host_semaphores: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// 获取主机级别的信号量
    async fn host_semaphore(&self, host: &str) -> Arc<Semaphore> {
        let mut map = self.host_semaphores.lock().await;
        map.entry(host.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.config.max_per_host as usize)))
            .clone()
    }

    /// 获取连接许可(全局 + 主机级别双重限制)
    pub async fn acquire(&self, host: &str) -> ConnectionPermit<'_> {
        let global_permit = self
            .global_semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("全局连接信号量已关闭");
        let host_sem = self.host_semaphore(host).await;
        let host_permit = host_sem
            .acquire_owned()
            .await
            .expect("主机连接信号量已关闭");
        self.active_count.fetch_add(1, Ordering::Relaxed);
        ConnectionPermit {
            _global_permit: global_permit,
            _host_permit: host_permit,
            active_count: &self.active_count,
        }
    }

    /// 当前活跃连接数
    pub fn active_connections(&self) -> u32 {
        self.active_count.load(Ordering::Relaxed)
    }

    /// 获取配置
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }
}

/// 连接许可,Drop 时自动归还连接
pub struct ConnectionPermit<'a> {
    _global_permit: tokio::sync::OwnedSemaphorePermit,
    _host_permit: tokio::sync::OwnedSemaphorePermit,
    active_count: &'a AtomicU32,
}

impl<'a> Drop for ConnectionPermit<'a> {
    fn drop(&mut self) {
        self.active_count.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pool_creation() {
        let pool = ConnectionPool::new(PoolConfig::default());
        assert_eq!(pool.active_connections(), 0);
    }

    #[tokio::test]
    async fn test_acquire_and_release() {
        let pool = ConnectionPool::new(PoolConfig {
            max_per_host: 2,
            max_global: 10,
        });
        {
            let _permit = pool.acquire("example.com").await;
            assert_eq!(pool.active_connections(), 1);
        }
        assert_eq!(pool.active_connections(), 0);
    }

    #[tokio::test]
    async fn test_host_limit() {
        let pool = Arc::new(ConnectionPool::new(PoolConfig {
            max_per_host: 2,
            max_global: 10,
        }));
        let _p1 = pool.acquire("example.com").await;
        let _p2 = pool.acquire("example.com").await;
        assert_eq!(pool.active_connections(), 2);
    }

    #[tokio::test]
    async fn test_different_hosts_independent() {
        let pool = ConnectionPool::new(PoolConfig {
            max_per_host: 1,
            max_global: 10,
        });
        let _p1 = pool.acquire("host1.com").await;
        let _p2 = pool.acquire("host2.com").await;
        assert_eq!(pool.active_connections(), 2);
    }

    #[test]
    fn test_default_config() {
        let config = PoolConfig::default();
        assert_eq!(config.max_per_host, 16);
        assert_eq!(config.max_global, 256);
    }
}
