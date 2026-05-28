//! 断点续传恢复管理
//!
//! 负责在应用启动时从持久化存储中恢复未完成的下载任务。

use serde::{Deserialize, Serialize};

use crate::kv::KvStore;

/// 下载任务持久化记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    /// 任务 ID
    pub task_id: String,
    /// 下载 URL
    pub url: String,
    /// 保存路径
    pub save_path: String,
    /// 文件总大小(字节)
    pub file_size: Option<u64>,
    /// 已下载字节数
    pub downloaded: u64,
    /// 已完成的分片索引列表
    pub completed_fragments: Vec<u32>,
    /// 分片总数
    pub total_fragments: u32,
    /// 任务状态
    pub status: String,
}

/// 恢复管理器
pub struct RecoveryManager {
    store: KvStore,
}

impl RecoveryManager {
    /// 创建恢复管理器
    pub fn new(store: KvStore) -> Self {
        Self { store }
    }

    /// 保存任务记录
    pub fn save_task(&self, record: &TaskRecord) -> std::io::Result<()> {
        self.store.put(&format!("task_{}", record.task_id), record)
    }

    /// 加载任务记录
    pub fn load_task(&self, task_id: &str) -> std::io::Result<Option<TaskRecord>> {
        self.store.get(&format!("task_{task_id}"))
    }

    /// 删除任务记录
    pub fn remove_task(&self, task_id: &str) -> std::io::Result<bool> {
        self.store.delete(&format!("task_{task_id}"))
    }

    /// 恢复所有未完成的任务
    pub fn recover_pending_tasks(&self) -> std::io::Result<Vec<TaskRecord>> {
        let mut pending = Vec::new();
        for key in self.store.keys()? {
            if let Some(task_id) = key.strip_prefix("task_") {
                if let Some(record) = self.load_task(task_id)? {
                    if record.status == "downloading" || record.status == "paused" {
                        pending.push(record);
                    }
                }
            }
        }
        Ok(pending)
    }

    /// 更新分片进度
    pub fn update_fragment_progress(
        &self,
        task_id: &str,
        fragment_index: u32,
        downloaded_bytes: u64,
    ) -> std::io::Result<()> {
        if let Some(mut record) = self.load_task(task_id)? {
            if !record.completed_fragments.contains(&fragment_index) {
                record.completed_fragments.push(fragment_index);
            }
            record.downloaded = downloaded_bytes;
            self.save_task(&record)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(task_id: &str, status: &str) -> TaskRecord {
        TaskRecord {
            task_id: task_id.to_string(),
            url: format!("https://example.com/{task_id}.zip"),
            save_path: format!("/downloads/{task_id}.zip"),
            file_size: Some(1024),
            downloaded: 512,
            completed_fragments: vec![0, 1],
            total_fragments: 4,
            status: status.to_string(),
        }
    }

    #[test]
    fn test_save_and_load_task() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        let mgr = RecoveryManager::new(store);
        let record = make_record("task-1", "downloading");
        mgr.save_task(&record).unwrap();
        let loaded = mgr.load_task("task-1").unwrap().unwrap();
        assert_eq!(loaded.task_id, "task-1");
        assert_eq!(loaded.downloaded, 512);
    }

    #[test]
    fn test_recover_pending_tasks() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        let mgr = RecoveryManager::new(store);
        mgr.save_task(&make_record("t1", "downloading")).unwrap();
        mgr.save_task(&make_record("t2", "completed")).unwrap();
        mgr.save_task(&make_record("t3", "paused")).unwrap();
        mgr.save_task(&make_record("t4", "failed")).unwrap();
        let pending = mgr.recover_pending_tasks().unwrap();
        assert_eq!(pending.len(), 2);
        let ids: Vec<&str> = pending.iter().map(|r| r.task_id.as_str()).collect();
        assert!(ids.contains(&"t1"));
        assert!(ids.contains(&"t3"));
    }

    #[test]
    fn test_update_fragment_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        let mgr = RecoveryManager::new(store);
        mgr.save_task(&make_record("t1", "downloading")).unwrap();
        mgr.update_fragment_progress("t1", 2, 768).unwrap();
        let record = mgr.load_task("t1").unwrap().unwrap();
        assert!(record.completed_fragments.contains(&2));
        assert_eq!(record.downloaded, 768);
    }

    #[test]
    fn test_remove_task() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        let mgr = RecoveryManager::new(store);
        mgr.save_task(&make_record("t1", "completed")).unwrap();
        assert!(mgr.remove_task("t1").unwrap());
        assert!(mgr.load_task("t1").unwrap().is_none());
    }

    #[test]
    fn test_load_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        let mgr = RecoveryManager::new(store);
        assert!(mgr.load_task("no-such-task").unwrap().is_none());
    }
}
