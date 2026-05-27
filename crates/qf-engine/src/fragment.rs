//! 分片引擎与状态机
//!
//! 管理单个分片的生命周期:Pending -> Downloading -> Verifying -> Writing -> Done
//! 支持失败重试(指数退避)和 EWMA 带宽追踪。

use std::time::Duration;

use bytes::Bytes;
use qf_core::types::FragmentInfo;

/// 分片状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentState {
    /// 等待下载
    Pending,
    /// 下载中
    Downloading,
    /// 校验中
    Verifying,
    /// 写入存储
    Writing,
    /// 已完成
    Done,
    /// 失败(可重试)
    Failed,
}

/// 分片下载记录
pub struct FragmentRecord {
    /// 分片信息
    pub info: FragmentInfo,
    /// 当前状态
    pub state: FragmentState,
    /// 已重试次数
    pub retry_count: u32,
    /// 最大重试次数
    pub max_retries: u32,
    /// 下载的数据
    pub data: Option<Bytes>,
    /// 最近一次下载耗时
    pub last_duration: Option<Duration>,
}

impl FragmentRecord {
    /// 创建新的分片记录
    pub fn new(info: FragmentInfo, max_retries: u32) -> Self {
        Self {
            info,
            state: FragmentState::Pending,
            retry_count: 0,
            max_retries,
            data: None,
            last_duration: None,
        }
    }

    /// 转换到下载中状态
    pub fn start_download(&mut self) {
        self.state = FragmentState::Downloading;
    }

    /// 下载完成,转换到校验状态
    pub fn complete_download(&mut self, data: Bytes, duration: Duration) {
        self.info.downloaded = data.len() as u64;
        self.data = Some(data);
        self.last_duration = Some(duration);
        self.state = FragmentState::Verifying;
    }

    /// 校验通过,转换到写入状态
    pub fn verify_ok(&mut self) {
        self.state = FragmentState::Writing;
    }

    /// 写入完成,转换到完成状态
    pub fn write_done(&mut self) {
        self.state = FragmentState::Done;
    }

    /// 标记失败,如果可重试则回到 Pending
    pub fn mark_failed(&mut self) -> bool {
        self.retry_count += 1;
        self.data = None;
        if self.retry_count <= self.max_retries {
            self.state = FragmentState::Pending;
            true
        } else {
            self.state = FragmentState::Failed;
            false
        }
    }

    /// 是否已完成
    pub fn is_done(&self) -> bool {
        self.state == FragmentState::Done
    }

    /// 是否已彻底失败(无法重试)
    pub fn is_failed(&self) -> bool {
        self.state == FragmentState::Failed
    }

    /// 计算重试退避时间(指数退避:1s, 2s, 4s, 8s, ...)
    pub fn backoff_duration(&self) -> Duration {
        Duration::from_secs(1u64 << self.retry_count.min(10))
    }
}

/// EWMA 带宽追踪器
pub struct BandwidthTracker {
    ewma: f64,
    alpha: f64,
    samples: Vec<u64>,
}

impl BandwidthTracker {
    /// 创建带宽追踪器
    /// - alpha: EWMA 平滑因子(0.0 ~ 1.0),越大越重视最新数据
    pub fn new(alpha: f64) -> Self {
        Self {
            ewma: 0.0,
            alpha: alpha.clamp(0.0, 1.0),
            samples: Vec::new(),
        }
    }

    /// 记录一个新的带宽样本(字节/秒)
    pub fn record(&mut self, bytes_per_sec: u64) {
        self.samples.push(bytes_per_sec);
        if self.samples.len() == 1 {
            self.ewma = bytes_per_sec as f64;
        } else {
            self.ewma = self.alpha * bytes_per_sec as f64 + (1.0 - self.alpha) * self.ewma;
        }
    }

    /// 获取当前 EWMA 带宽估计(字节/秒)
    pub fn estimate(&self) -> u64 {
        self.ewma as u64
    }

    /// 获取采样数
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

impl Default for BandwidthTracker {
    fn default() -> Self {
        Self::new(0.3)
    }
}

/// 根据带宽和文件大小计算最优分片大小
pub fn compute_fragment_size(
    file_size: u64,
    bandwidth_bps: u64,
    min_size: u64,
    max_size: u64,
    target_fragments: u32,
) -> u64 {
    if file_size == 0 {
        return 0;
    }

    // 基础分片大小 = 文件大小 / 目标分片数
    let base = file_size / target_fragments.max(1) as u64;

    // 根据带宽调整:高带宽时增大分片以减少开销
    let bandwidth_factor = if bandwidth_bps > 100 * 1024 * 1024 {
        2.0 // > 100Mbps,分片翻倍
    } else if bandwidth_bps > 10 * 1024 * 1024 {
        1.5 // > 10Mbps
    } else {
        1.0
    };

    let adjusted = (base as f64 * bandwidth_factor) as u64;
    adjusted.clamp(min_size, max_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qf_core::types::FragmentInfo;

    fn make_frag(index: u32, size: u64) -> FragmentInfo {
        FragmentInfo {
            index,
            start: index as u64 * size,
            end: (index as u64 + 1) * size - 1,
            size,
            downloaded: 0,
            hash: None,
        }
    }

    #[test]
    fn test_fragment_state_transitions() {
        let info = make_frag(0, 1024);
        let mut record = FragmentRecord::new(info, 3);
        assert_eq!(record.state, FragmentState::Pending);

        record.start_download();
        assert_eq!(record.state, FragmentState::Downloading);

        record.complete_download(Bytes::from_static(b"test"), Duration::from_millis(100));
        assert_eq!(record.state, FragmentState::Verifying);

        record.verify_ok();
        assert_eq!(record.state, FragmentState::Writing);

        record.write_done();
        assert_eq!(record.state, FragmentState::Done);
        assert!(record.is_done());
    }

    #[test]
    fn test_fragment_retry() {
        let info = make_frag(0, 1024);
        let mut record = FragmentRecord::new(info, 2);

        record.start_download();
        assert!(record.mark_failed()); // retry 1
        assert_eq!(record.state, FragmentState::Pending);

        record.start_download();
        assert!(record.mark_failed()); // retry 2
        assert_eq!(record.state, FragmentState::Pending);

        record.start_download();
        assert!(!record.mark_failed()); // retry 3, exceeds max
        assert_eq!(record.state, FragmentState::Failed);
        assert!(record.is_failed());
    }

    #[test]
    fn test_backoff_duration() {
        let info = make_frag(0, 1024);
        let mut record = FragmentRecord::new(info, 5);

        record.retry_count = 0;
        assert_eq!(record.backoff_duration(), Duration::from_secs(1));

        record.retry_count = 1;
        assert_eq!(record.backoff_duration(), Duration::from_secs(2));

        record.retry_count = 2;
        assert_eq!(record.backoff_duration(), Duration::from_secs(4));

        record.retry_count = 3;
        assert_eq!(record.backoff_duration(), Duration::from_secs(8));
    }

    #[test]
    fn test_bandwidth_tracker() {
        let mut tracker = BandwidthTracker::new(0.5);
        tracker.record(100);
        assert_eq!(tracker.estimate(), 100);

        tracker.record(200);
        // EWMA = 0.5 * 200 + 0.5 * 100 = 150
        assert_eq!(tracker.estimate(), 150);

        tracker.record(300);
        // EWMA = 0.5 * 300 + 0.5 * 150 = 225
        assert_eq!(tracker.estimate(), 225);
    }

    #[test]
    fn test_bandwidth_tracker_default() {
        let mut tracker = BandwidthTracker::default();
        tracker.record(1000);
        assert_eq!(tracker.sample_count(), 1);
    }

    #[test]
    fn test_compute_fragment_size_normal() {
        let size = compute_fragment_size(
            100 * 1024 * 1024,
            1024 * 1024,
            1024 * 1024,
            64 * 1024 * 1024,
            16,
        );
        assert!(size >= 1024 * 1024);
        assert!(size <= 64 * 1024 * 1024);
    }

    #[test]
    fn test_compute_fragment_size_high_bandwidth() {
        let size = compute_fragment_size(
            1024 * 1024 * 1024,
            200 * 1024 * 1024,
            1024 * 1024,
            64 * 1024 * 1024,
            16,
        );
        assert!(size >= 1024 * 1024);
    }

    #[test]
    fn test_compute_fragment_size_zero() {
        let size = compute_fragment_size(0, 0, 1024, 64 * 1024 * 1024, 16);
        assert_eq!(size, 0);
    }

    #[test]
    fn test_compute_fragment_size_small_file() {
        let size = compute_fragment_size(500, 1024, 1024, 64 * 1024 * 1024, 4);
        assert_eq!(size, 1024); // clamp to min
    }
}
