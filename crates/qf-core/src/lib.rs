//! QuantumFetch 核心类型、trait 定义与错误体系
//!
//! 本 crate 定义所有模块共享的公共接口,包括:
//! - 下载任务、分片、协议、存储、校验的 trait 抽象
//! - 统一错误类型
//! - 配置类型
//! - 事件类型

pub mod config;
pub mod error;
pub mod event;
pub mod types;

// 重新导出核心类型
pub use config::{ConnectionConfig, DownloadConfig, SchedulerConfig};
pub use error::{QfError, QfResult};
pub use event::{DownloadEvent, FragmentEvent, PeerEvent};
pub use types::{DownloadState, FileMetadata, FragmentInfo, TaskId};
