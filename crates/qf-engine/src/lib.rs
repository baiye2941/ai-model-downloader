//! QuantumFetch 引擎层:分片引擎、连接管理
//!
//! 核心下载引擎实现:
//! - 超分片策略(动态粒度调整)
//! - 连接池管理
//! - 分片状态机
//! - 并发控制

pub mod connection;
pub mod downloader;
pub mod fragment;
pub mod orchestrator;

pub use connection::{ConnectionPool, PoolConfig};
pub use downloader::{DownloadTask, ProtocolKind, StorageKind, VerifierKind};
pub use fragment::{BandwidthTracker, FragmentRecord, FragmentState};
pub use orchestrator::DownloadOrchestrator;
