//! 事件类型定义

use serde::{Deserialize, Serialize};

use crate::types::{DownloadState, TaskId};

/// 下载任务事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DownloadEvent {
    /// 任务状态变更
    StateChanged {
        task_id: TaskId,
        old_state: DownloadState,
        new_state: DownloadState,
    },
    /// 进度更新
    Progress {
        task_id: TaskId,
        /// 已下载字节数
        downloaded: u64,
        /// 总字节数
        total: Option<u64>,
        /// 当前速度(字节/秒)
        speed: u64,
    },
    /// 任务完成
    Completed {
        task_id: TaskId,
        /// 文件路径
        file_path: String,
        /// 平均速度(字节/秒)
        avg_speed: u64,
        /// 耗时(毫秒)
        elapsed_ms: u64,
    },
    /// 任务失败
    Failed {
        task_id: TaskId,
        /// 错误信息
        error: String,
    },
}

/// 分片事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FragmentEvent {
    /// 分片开始下载
    Started {
        task_id: TaskId,
        fragment_index: u32,
    },
    /// 分片进度
    Progress {
        task_id: TaskId,
        fragment_index: u32,
        downloaded: u64,
        total: u64,
    },
    /// 分片完成
    Completed {
        task_id: TaskId,
        fragment_index: u32,
    },
    /// 分片失败(将重试)
    Failed {
        task_id: TaskId,
        fragment_index: u32,
        error: String,
        retry_count: u32,
    },
}

/// Peer 事件(P2SP)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerEvent {
    /// 发现新 Peer
    Discovered { task_id: TaskId, peer_addr: String },
    /// Peer 连接建立
    Connected { task_id: TaskId, peer_addr: String },
    /// Peer 连接断开
    Disconnected {
        task_id: TaskId,
        peer_addr: String,
        reason: String,
    },
}
