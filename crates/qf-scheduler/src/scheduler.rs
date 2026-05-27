//! 任务调度器
//!
//! 管理下载任务的优先级队列和连接分配策略。

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use qf_core::TaskId;

/// 任务优先级
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// 预取(最低)
    Prefetch = 0,
    /// 队列下载
    Queue = 1,
    /// 用户主动下载(最高)
    UserInitiated = 2,
}

/// 调度队列中的任务条目
#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub task_id: TaskId,
    pub priority: Priority,
    /// 文件大小(用于小文件优先策略)
    pub file_size: u64,
    /// 当前进度(0.0 ~ 1.0),即将完成的任务提升优先级
    pub progress: f64,
}

impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool {
        self.task_id == other.task_id
    }
}

impl Eq for ScheduledTask {}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // 按优先级 -> 进度(即将完成优先) -> 文件大小(小文件优先)排序
        self.priority
            .cmp(&other.priority)
            .then_with(|| {
                self.progress
                    .partial_cmp(&other.progress)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| other.file_size.cmp(&self.file_size))
    }
}

/// 下载任务调度器
pub struct Scheduler {
    queue: BinaryHeap<ScheduledTask>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            queue: BinaryHeap::new(),
        }
    }

    /// 添加任务到调度队列
    pub fn push(&mut self, task: ScheduledTask) {
        self.queue.push(task);
    }

    /// 取出优先级最高的任务
    pub fn pop(&mut self) -> Option<ScheduledTask> {
        self.queue.pop()
    }

    /// 队列中任务数
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// 队列是否为空
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(priority: Priority, size: u64, progress: f64) -> ScheduledTask {
        ScheduledTask {
            task_id: TaskId::new_v4(),
            priority,
            file_size: size,
            progress,
        }
    }

    #[test]
    fn test_scheduler_empty() {
        let sched = Scheduler::new();
        assert!(sched.is_empty());
        assert_eq!(sched.len(), 0);
    }

    #[test]
    fn test_user_priority_first() {
        let mut sched = Scheduler::new();
        sched.push(make_task(Priority::Prefetch, 1000, 0.0));
        sched.push(make_task(Priority::UserInitiated, 1000, 0.0));
        sched.push(make_task(Priority::Queue, 1000, 0.0));

        let top = sched.pop().unwrap();
        assert_eq!(top.priority, Priority::UserInitiated);
    }

    #[test]
    fn test_small_file_priority() {
        let mut sched = Scheduler::new();
        sched.push(make_task(Priority::Queue, 10000, 0.0));
        sched.push(make_task(Priority::Queue, 100, 0.0));

        let top = sched.pop().unwrap();
        assert_eq!(top.file_size, 100);
    }

    #[test]
    fn test_progress_priority() {
        let mut sched = Scheduler::new();
        sched.push(make_task(Priority::Queue, 1000, 0.1));
        sched.push(make_task(Priority::Queue, 1000, 0.9));

        let top = sched.pop().unwrap();
        assert!(top.progress > 0.5);
    }

    #[test]
    fn test_fifo_within_same_priority() {
        let mut sched = Scheduler::new();
        let t1 = make_task(Priority::Queue, 1000, 0.0);
        let t2 = make_task(Priority::Queue, 1000, 0.0);
        sched.push(t1.clone());
        sched.push(t2.clone());

        // 相同优先级、大小、进度时,按 BinaryHeap 的内部顺序
        let _first = sched.pop().unwrap();
        let _second = sched.pop().unwrap();
        assert!(sched.is_empty());
    }
}
