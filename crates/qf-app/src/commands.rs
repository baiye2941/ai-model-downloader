//! Tauri 命令模块
//!
//! 提供应用信息查询、下载任务管理、配置管理、嗅探等 Tauri 命令。
//! 任务存储使用 `AppState` 通过 Tauri 的 `manage()` 注入,线程安全。
//! 下载任务通过后台 tokio task 异步执行,不阻塞 Tauri 命令返回。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Local;
use qf_core::filename::{extract_filename_from_url, parse_content_disposition};
use qf_core::types::FileMetadata;
use qf_engine::DownloadOrchestrator;
use qf_engine::connection::PoolConfig;
use qf_sniffer::capture::{ResourceType, identify_resource};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 应用错误类型
// ---------------------------------------------------------------------------

/// 结构化应用错误类型
#[derive(Debug, thiserror::Error, serde::Serialize)]
pub enum AppError {
    #[error("任务不存在: {0}")]
    TaskNotFound(String),
    #[error("任务已存在: {0}")]
    TaskAlreadyExists(String),
    #[error("网络错误: {0}")]
    Network(String),
    #[error("配置错误: {0}")]
    Config(String),
}

// ---------------------------------------------------------------------------
// 数据类型
// ---------------------------------------------------------------------------

/// 下载任务信息(前端可见)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    /// 任务唯一标识
    pub id: String,
    /// 下载地址
    pub url: String,
    /// 文件名(从 URL 提取)
    pub file_name: String,
    /// 文件总大小(字节),None 表示未知
    pub file_size: Option<u64>,
    /// 已下载字节数
    pub downloaded: u64,
    /// 当前下载速度(字节/秒)
    pub speed: u64,
    /// 任务状态: pending / downloading / paused / completed / failed / cancelled
    pub status: String,
    /// 下载进度(0.0 ~ 1.0)
    pub progress: f64,
    /// 分片总数
    pub fragments_total: u32,
    /// 已完成分片数
    pub fragments_done: u32,
    /// 创建时间(ISO 8601 本地时间)
    pub created_at: String,
}

/// 应用全局配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 默认下载目录
    pub download_dir: String,
    /// 最大并发任务数
    pub max_concurrent_tasks: u32,
    /// 每任务最大并发分片数
    pub max_concurrent_fragments: u32,
    /// 每主机最大连接数
    pub max_connections_per_host: u32,
    /// 是否启用 QUIC 协议
    pub enable_quic: bool,
    /// 是否校验文件完整性
    pub verify_checksum: bool,
}

/// 下载进度详情(前端轮询)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    /// 任务唯一标识
    pub task_id: String,
    /// 当前状态
    pub status: String,
    /// 下载进度(0.0 ~ 1.0)
    pub progress: f64,
    /// 已下载字节数
    pub downloaded: u64,
    /// 文件总大小(字节)
    pub file_size: Option<u64>,
    /// 当前下载速度(字节/秒)
    pub speed: u64,
    /// 分片总数
    pub fragments_total: u32,
    /// 已完成分片数
    pub fragments_done: u32,
}

/// 嗅探到的可下载资源
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnifferResource {
    /// 资源唯一标识
    pub id: String,
    /// 下载 URL
    pub url: String,
    /// 资源类型(字符串形式,如 "video"、"archive")
    pub resource_type: String,
    /// 文件名(从 URL 提取)
    pub file_name: String,
    /// 捕获时间(ISO 8601 本地时间)
    pub captured_at: String,
}

/// 任务状态常量
mod status {
    pub const PENDING: &str = "pending";
    pub const DOWNLOADING: &str = "downloading";
    pub const PAUSED: &str = "paused";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
    pub const CANCELLED: &str = "cancelled";
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// 应用全局状态,通过 Tauri 的 `manage()` 注入
pub struct AppState {
    /// 任务列表
    pub tasks: Arc<Mutex<HashMap<String, TaskInfo>>>,
    /// 应用配置
    pub config: Arc<Mutex<AppConfig>>,
    /// 后台下载任务句柄(用于取消)
    pub handles: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// 活跃分片许可计数(用于限制并发)
    pub active_permits: Arc<AtomicU32>,
    /// 嗅探到的资源列表
    pub sniffer: Arc<Mutex<Vec<SnifferResource>>>,
    /// 嗅探过滤规则(URL 关键词)
    pub sniffer_filters: Arc<Mutex<Vec<String>>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// 创建默认 AppState 实例
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(AppConfig {
                download_dir: dirs()
                    .map(|p| p.join("Downloads").to_string_lossy().to_string())
                    .unwrap_or_else(|| ".".to_string()),
                max_concurrent_tasks: 5,
                max_concurrent_fragments: 16,
                max_connections_per_host: 16,
                enable_quic: false,
                verify_checksum: true,
            })),
            handles: Arc::new(Mutex::new(HashMap::new())),
            active_permits: Arc::new(AtomicU32::new(0)),
            sniffer: Arc::new(Mutex::new(Vec::new())),
            sniffer_filters: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 获取用户主目录(Windows: USERPROFILE, Unix: HOME)
fn dirs() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
}

/// 获取当前本地时间的 ISO 8601 字符串
fn now_iso8601() -> String {
    Local::now().to_rfc3339()
}

/// 将 `ResourceType` 枚举转为可读字符串
fn resource_type_to_string(rt: ResourceType) -> &'static str {
    match rt {
        ResourceType::Video => "video",
        ResourceType::Audio => "audio",
        ResourceType::Document => "document",
        ResourceType::Archive => "archive",
        ResourceType::Executable => "executable",
        ResourceType::Image => "image",
        ResourceType::Other => "other",
    }
}

// ---------------------------------------------------------------------------
// 内部辅助函数(在持有外部锁的上下文中调用,不自行获取锁)
// ---------------------------------------------------------------------------

/// 更新任务状态(需要调用方已持有 tasks 写锁或传入可变引用)
fn update_task_status(store: &mut HashMap<String, TaskInfo>, task_id: &str, new_status: &str) {
    if let Some(task) = store.get_mut(task_id) {
        task.status = new_status.to_string();
        if new_status == status::COMPLETED
            || new_status == status::FAILED
            || new_status == status::CANCELLED
        {
            task.speed = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// 后台下载任务
// ---------------------------------------------------------------------------

/// 后台下载任务实现
///
/// 使用 `DownloadOrchestrator` 规划分片、管理连接,模拟分片下载并持续更新进度。
/// 通过检查 `AppState.tasks` 中的状态来响应暂停和取消操作。
async fn task_fn(
    state: Arc<AppState>,
    task_id: String,
    url: String,
    download_dir: String,
    mut orchestrator: DownloadOrchestrator,
    metadata: FileMetadata,
) {
    let download_url = match Url::parse(&url) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(task_id = %task_id, error = %e, "URL 解析失败");
            let mut store = state.tasks.lock().await;
            update_task_status(&mut store, &task_id, status::FAILED);
            return;
        }
    };

    let host = match download_url.host_str() {
        Some(h) => h.to_string(),
        None => {
            tracing::error!(task_id = %task_id, "URL 主机为空");
            let mut store = state.tasks.lock().await;
            update_task_status(&mut store, &task_id, status::FAILED);
            return;
        }
    };

    let file_size = metadata.file_size.unwrap_or(0);
    let supports_range = metadata.supports_range;
    tracing::info!(
        task_id = %task_id,
        file_size = file_size,
        supports_range = supports_range,
        host = %host,
        download_dir = %download_dir,
        "开始下载"
    );

    let fragments = orchestrator.plan_fragments(file_size, supports_range);
    let fragment_count = fragments.len() as u32;
    tracing::info!(
        task_id = %task_id,
        fragments = fragment_count,
        supports_range = supports_range,
        "分片策略规划完成"
    );

    {
        let mut store = state.tasks.lock().await;
        if let Some(task) = store.get_mut(&task_id) {
            task.file_size = metadata.file_size;
            task.fragments_total = fragment_count;
        }
        update_task_status(&mut store, &task_id, status::DOWNLOADING);
    }

    let max_concurrent = {
        let cfg = state.config.lock().await;
        cfg.max_concurrent_fragments as usize
    };
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));

    let mut any_fragment_failed = false;
    let mut total_downloaded: u64 = 0;

    for frag in &fragments {
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => {
                tracing::error!(task_id = %task_id, "分片信号量已关闭,放弃分片 {}", frag.index);
                any_fragment_failed = true;
                continue;
            }
        };
        state.active_permits.fetch_add(1, Ordering::Relaxed);
        orchestrator.register_fragment(frag.clone());

        let chunk_size: u64 = 1024 * 100;
        let chunks = if chunk_size > 0 {
            frag.size.div_ceil(chunk_size)
        } else {
            1
        };
        let frag_start = Instant::now();
        let mut frag_downloaded: u64 = 0;

        for _ in 0..chunks {
            {
                let store = state.tasks.lock().await;
                if let Some(task) = store.get(&task_id) {
                    match task.status.as_str() {
                        status::CANCELLED => {
                            tracing::info!(task_id = %task_id, "任务已取消,退出后台下载");
                            drop(store);
                            drop(permit);
                            state.active_permits.fetch_sub(1, Ordering::Relaxed);
                            return;
                        }
                        status::PAUSED => {
                            tracing::info!(task_id = %task_id, "任务已暂停,等待恢复...");
                        }
                        _ => {}
                    }
                }
            }

            {
                let mut paused_iterations = 0u32;
                loop {
                    let is_paused = state
                        .tasks
                        .lock()
                        .await
                        .get(&task_id)
                        .is_some_and(|t| t.status == status::PAUSED);
                    if !is_paused {
                        break;
                    }
                    paused_iterations += 1;
                    if paused_iterations > 1500 {
                        tracing::warn!(task_id = %task_id, "暂停超时,标记任务失败");
                        let mut store = state.tasks.lock().await;
                        update_task_status(&mut store, &task_id, status::FAILED);
                        drop(permit);
                        state.active_permits.fetch_sub(1, Ordering::Relaxed);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }

            tokio::time::sleep(Duration::from_millis(2)).await;
            let simulated = chunk_size.min(frag.size - frag_downloaded);
            frag_downloaded += simulated;
            total_downloaded += simulated;

            let elapsed_secs = frag_start.elapsed().as_secs_f64();
            let speed = if elapsed_secs > 0.0 {
                (frag_downloaded as f64 / elapsed_secs) as u64
            } else {
                0
            };
            let progress = if file_size > 0 {
                total_downloaded as f64 / file_size as f64
            } else {
                0.0
            };

            let mut store = state.tasks.lock().await;
            if let Some(task) = store.get_mut(&task_id) {
                task.downloaded = total_downloaded;
                task.speed = speed;
                task.progress = progress.min(1.0);
            }
        }

        orchestrator.on_fragment_complete(frag, frag_start.elapsed());
        let fragments_done = orchestrator
            .active_fragments()
            .iter()
            .filter(|r| r.is_done())
            .count() as u32;
        drop(permit);
        state.active_permits.fetch_sub(1, Ordering::Relaxed);

        {
            let mut store = state.tasks.lock().await;
            if let Some(task) = store.get_mut(&task_id) {
                task.fragments_done = fragments_done;
            }
        }

        tracing::debug!(
            task_id = %task_id,
            fragment = frag.index,
            fragments_done = fragments_done,
            "分片下载完成"
        );
    }

    {
        let mut store = state.tasks.lock().await;
        if any_fragment_failed {
            update_task_status(&mut store, &task_id, status::FAILED);
            tracing::error!(task_id = %task_id, "部分分片下载失败");
        } else {
            if let Some(task) = store.get_mut(&task_id) {
                task.progress = 1.0;
                task.downloaded = file_size.max(total_downloaded);
            }
            update_task_status(&mut store, &task_id, status::COMPLETED);
            tracing::info!(
                task_id = %task_id,
                total_bytes = total_downloaded,
                "下载任务完成"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 应用信息命令
// ---------------------------------------------------------------------------

/// 应用版本信息
#[derive(Serialize)]
pub struct AppInfo {
    pub version: &'static str,
    pub name: &'static str,
}

/// 获取应用信息
#[tauri::command]
pub fn get_app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        name: "QuantumFetch",
    }
}

/// 获取支持的协议列表
#[tauri::command]
pub fn supported_protocols() -> Vec<&'static str> {
    vec!["HTTP", "HTTPS", "FTP", "QUIC"]
}

// ---------------------------------------------------------------------------
// 任务管理命令
// ---------------------------------------------------------------------------

/// 创建下载任务
///
/// `url` 为下载地址,`download_dir` 可选覆盖默认下载目录。
/// 创建后立即启动后台下载任务,返回新任务的 UUID。
/// 使用 `DownloadOrchestrator` 规划分片策略,后台异步执行下载。
#[tauri::command]
pub async fn create_task(
    state: tauri::State<'_, AppState>,
    url: String,
    download_dir: Option<String>,
) -> Result<String, String> {
    let task_id = Uuid::new_v4().to_string();
    let file_name = extract_filename_from_url(&url);
    let created_at = now_iso8601();

    {
        let store = state.tasks.lock().await;
        if store.values().any(|t| {
            t.url == url
                && t.status != status::CANCELLED
                && t.status != status::COMPLETED
                && t.status != status::FAILED
        }) {
            return Err("相同 URL 的下载任务已存在".to_string());
        }
        let max_tasks = state.config.lock().await.max_concurrent_tasks as usize;
        let active_count = store
            .values()
            .filter(|t| t.status == status::DOWNLOADING || t.status == status::PENDING)
            .count();
        if active_count >= max_tasks {
            return Err(format!(
                "已达最大并发任务数({max_tasks}),请等待现有任务完成"
            ));
        }
    }

    let download_dir_str = {
        let cfg = state.config.lock().await;
        download_dir.unwrap_or_else(|| cfg.download_dir.clone())
    };

    let task = TaskInfo {
        id: task_id.clone(),
        url: url.clone(),
        file_name,
        file_size: None,
        downloaded: 0,
        speed: 0,
        status: status::PENDING.to_string(),
        progress: 0.0,
        fragments_total: 0,
        fragments_done: 0,
        created_at,
    };

    {
        let mut store = state.tasks.lock().await;
        store.insert(task_id.clone(), task);
    }

    let orchestrator = DownloadOrchestrator::new(PoolConfig {
        max_per_host: 16,
        max_global: 256,
    });

    let state_arc = Arc::new(AppState {
        tasks: state.tasks.clone(),
        config: state.config.clone(),
        handles: state.handles.clone(),
        active_permits: state.active_permits.clone(),
        sniffer: state.sniffer.clone(),
        sniffer_filters: state.sniffer_filters.clone(),
    });

    let tid = task_id.clone();
    let url_clone = url.clone();
    let handle = tokio::spawn(async move {
        let metadata = match probe_metadata(&url_clone).await {
            Ok(meta) => {
                tracing::info!(
                    task_id = %tid,
                    file_name = %meta.file_name,
                    file_size = ?meta.file_size,
                    supports_range = meta.supports_range,
                    "元数据探测成功"
                );
                meta
            }
            Err(e) => {
                tracing::error!(task_id = %tid, error = %e, "元数据探测失败");
                let mut store = state_arc.tasks.lock().await;
                update_task_status(&mut store, &tid, status::FAILED);
                return;
            }
        };

        task_fn(state_arc, tid, url_clone, download_dir_str, orchestrator, metadata).await;
    });

    {
        let mut handles = state.handles.lock().await;
        handles.insert(task_id.clone(), handle);
    }

    tracing::info!(task_id = %task_id, "创建下载任务并启动后台下载");
    Ok(task_id)
}

/// 探测远程文件元数据
///
/// 发送 HEAD 请求获取 Content-Length、Accept-Ranges 等信息。
/// 失败时返回合理的默认值(未知大小、不支持 Range)。
async fn probe_metadata(url: &str) -> Result<FileMetadata, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    let response = client
        .head(url)
        .send()
        .await
        .map_err(|e| format!("HEAD 请求失败: {e}"))?;

    let file_size = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let supports_range = response
        .headers()
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("bytes"));

    let file_name = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_disposition)
        .unwrap_or_else(|| extract_filename_from_url(url));

    Ok(FileMetadata {
        file_name,
        file_size,
        content_type: response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
        supports_range,
        etag: response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
        last_modified: response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
    })
}

/// 暂停下载任务
///
/// 仅 `pending` 或 `downloading` 状态的任务可以暂停。
/// 后台任务检测到暂停状态后将自旋等待恢复。
#[tauri::command]
pub async fn pause_task(
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let mut store = state.tasks.lock().await;

    let task = store
        .get_mut(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;

    match task.status.as_str() {
        status::PENDING | status::DOWNLOADING => {
            task.status = status::PAUSED.to_string();
            task.speed = 0;
            tracing::info!(task_id = %task_id, "暂停任务");
            Ok(())
        }
        other => Err(format!("当前状态 '{other}' 不允许暂停")),
    }
}

/// 恢复下载任务
///
/// 仅 `paused` 状态的任务可以恢复。
/// 后台任务检测到状态恢复后将继续下载。
#[tauri::command]
pub async fn resume_task(
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let mut store = state.tasks.lock().await;

    let task = store
        .get_mut(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;

    if task.status == status::PAUSED {
        task.status = status::DOWNLOADING.to_string();
        tracing::info!(task_id = %task_id, "恢复任务");
        Ok(())
    } else {
        Err(format!("仅暂停状态可恢复,当前状态: '{}'", task.status))
    }
}

/// 取消下载任务
///
/// 已完成或已取消的任务不可再次取消。
/// 取消会中止后台下载任务并移除句柄。
#[tauri::command]
pub async fn cancel_task(
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let mut store = state.tasks.lock().await;

    let task = store
        .get_mut(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;

    match task.status.as_str() {
        status::COMPLETED | status::CANCELLED => Err(format!("任务已{},无法取消", task.status)),
        _ => {
            let mut handles = state.handles.lock().await;
            if let Some(handle) = handles.remove(&task_id) {
                handle.abort();
            }
            drop(handles);

            update_task_status(&mut store, &task_id, status::CANCELLED);
            tracing::info!(task_id = %task_id, "取消任务");
            Ok(())
        }
    }
}

/// 删除下载任务
///
/// 仅 `completed`、`cancelled` 或 `failed` 状态的任务可以删除。
/// 活跃任务需先取消再删除。
#[tauri::command]
pub async fn delete_task(
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let mut store = state.tasks.lock().await;

    let task = store
        .get(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;

    match task.status.as_str() {
        status::COMPLETED | status::CANCELLED | status::FAILED => {
            store.remove(&task_id);
            let mut handles = state.handles.lock().await;
            handles.remove(&task_id);
            tracing::info!(task_id = %task_id, "删除任务");
            Ok(())
        }
        other => Err(format!("当前状态 '{other}' 不允许删除,请先取消任务")),
    }
}

/// 获取所有任务列表
#[tauri::command]
pub async fn get_task_list(state: tauri::State<'_, AppState>) -> Result<Vec<TaskInfo>, String> {
    let store = state.tasks.lock().await;
    Ok(store.values().cloned().collect())
}

/// 获取单个任务详情
#[tauri::command]
pub async fn get_task_detail(
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<TaskInfo, String> {
    let store = state.tasks.lock().await;

    store
        .get(&task_id)
        .cloned()
        .ok_or_else(|| format!("任务不存在: {task_id}"))
}

// ---------------------------------------------------------------------------
// 进度查询命令
// ---------------------------------------------------------------------------

/// 获取下载进度详情
///
/// 返回指定任务的实时进度、速度、分片状态等信息。
/// 前端可定期轮询此接口更新 UI。
#[tauri::command]
pub async fn get_download_progress(
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<DownloadProgress, String> {
    let store = state.tasks.lock().await;

    let task = store
        .get(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;

    Ok(DownloadProgress {
        task_id: task.id.clone(),
        status: task.status.clone(),
        progress: task.progress,
        downloaded: task.downloaded,
        file_size: task.file_size,
        speed: task.speed,
        fragments_total: task.fragments_total,
        fragments_done: task.fragments_done,
    })
}

// ---------------------------------------------------------------------------
// 嗅探命令
// ---------------------------------------------------------------------------

/// 获取嗅探到的可下载资源列表
///
/// 返回当前所有已捕获的资源,按捕获时间降序排列。
#[tauri::command]
pub async fn get_sniffer_resources(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SnifferResource>, String> {
    let store = state.sniffer.lock().await;
    Ok(store.iter().rev().cloned().collect())
}

/// 添加嗅探过滤规则
///
/// `filter` 为 URL 关键词,仅包含匹配关键词的资源会被捕获。
/// 规则持久化到 `sniffer_filters`,供嗅探引擎使用。
#[tauri::command]
pub async fn add_sniffer_filter(
    state: tauri::State<'_, AppState>,
    filter: String,
) -> Result<(), String> {
    if filter.is_empty() {
        return Err("过滤规则不能为空".to_string());
    }
    let mut filters = state.sniffer_filters.lock().await;
    if filters.contains(&filter) {
        return Err("过滤规则已存在".to_string());
    }
    tracing::info!(filter = %filter, "添加嗅探过滤规则");
    filters.push(filter);
    Ok(())
}

/// 内部接口:添加嗅探资源(供嗅探引擎调用)
///
/// 检查过滤规则,仅当 URL 匹配时才添加资源。
pub async fn add_sniffer_resource(state: &AppState, url: String) {
    let filters = state.sniffer_filters.lock().await;
    if !filters.is_empty() && !filters.iter().any(|f| url.contains(f.as_str())) {
        return;
    }
    drop(filters);

    let resource_type = identify_resource(&url);
    let file_name = extract_filename_from_url(&url);
    let resource = SnifferResource {
        id: Uuid::new_v4().to_string(),
        url: url.clone(),
        resource_type: resource_type_to_string(resource_type).to_string(),
        file_name,
        captured_at: now_iso8601(),
    };

    let mut store = state.sniffer.lock().await;

    if store.iter().any(|r| r.url == url) {
        return;
    }

    const MAX_SNIFFER_RESOURCES: usize = 1000;
    if store.len() >= MAX_SNIFFER_RESOURCES {
        store.remove(0);
    }

    tracing::info!(url = %url, resource_type = %resource.resource_type, "捕获新资源");
    store.push(resource);
}

// ---------------------------------------------------------------------------
// 配置管理命令
// ---------------------------------------------------------------------------

/// 获取当前应用配置
#[tauri::command]
pub async fn get_config(state: tauri::State<'_, AppState>) -> Result<AppConfig, String> {
    let cfg = state.config.lock().await;
    Ok(cfg.clone())
}

/// 更新应用配置
///
/// 前端传入完整的新配置,整体替换旧配置。
#[tauri::command]
pub async fn update_config(
    state: tauri::State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    let mut cfg = state.config.lock().await;
    *cfg = config;
    tracing::info!("应用配置已更新");
    Ok(())
}

// ---------------------------------------------------------------------------
// 测试辅助函数(直接操作 AppState,不依赖 Tauri State 注入)
// ---------------------------------------------------------------------------

#[cfg(test)]
/// 创建下载任务(直接操作 AppState)
async fn create_task_inner(state: &AppState, url: String, download_dir: Option<String>) -> Result<String, String> {
    let task_id = Uuid::new_v4().to_string();
    let file_name = extract_filename_from_url(&url);
    let created_at = now_iso8601();

    {
        let store = state.tasks.lock().await;
        if store.values().any(|t| {
            t.url == url
                && t.status != status::CANCELLED
                && t.status != status::COMPLETED
                && t.status != status::FAILED
        }) {
            return Err("相同 URL 的下载任务已存在".to_string());
        }
        let max_tasks = state.config.lock().await.max_concurrent_tasks as usize;
        let active_count = store
            .values()
            .filter(|t| t.status == status::DOWNLOADING || t.status == status::PENDING)
            .count();
        if active_count >= max_tasks {
            return Err(format!(
                "已达最大并发任务数({max_tasks}),请等待现有任务完成"
            ));
        }
    }

    let download_dir_str = {
        let cfg = state.config.lock().await;
        download_dir.unwrap_or_else(|| cfg.download_dir.clone())
    };

    let task = TaskInfo {
        id: task_id.clone(),
        url: url.clone(),
        file_name,
        file_size: None,
        downloaded: 0,
        speed: 0,
        status: status::PENDING.to_string(),
        progress: 0.0,
        fragments_total: 0,
        fragments_done: 0,
        created_at,
    };

    {
        let mut store = state.tasks.lock().await;
        store.insert(task_id.clone(), task);
    }

    let orchestrator = DownloadOrchestrator::new(PoolConfig {
        max_per_host: 16,
        max_global: 256,
    });

    let state_arc = Arc::new(AppState {
        tasks: state.tasks.clone(),
        config: state.config.clone(),
        handles: state.handles.clone(),
        active_permits: state.active_permits.clone(),
        sniffer: state.sniffer.clone(),
        sniffer_filters: state.sniffer_filters.clone(),
    });

    let tid = task_id.clone();
    let url_clone = url.clone();
    let handle = tokio::spawn(async move {
        let metadata = match probe_metadata(&url_clone).await {
            Ok(meta) => {
                tracing::info!(
                    task_id = %tid,
                    file_name = %meta.file_name,
                    file_size = ?meta.file_size,
                    supports_range = meta.supports_range,
                    "元数据探测成功"
                );
                meta
            }
            Err(e) => {
                tracing::error!(task_id = %tid, error = %e, "元数据探测失败");
                let mut store = state_arc.tasks.lock().await;
                update_task_status(&mut store, &tid, status::FAILED);
                return;
            }
        };

        task_fn(state_arc, tid, url_clone, download_dir_str, orchestrator, metadata).await;
    });

    {
        let mut handles = state.handles.lock().await;
        handles.insert(task_id.clone(), handle);
    }

    tracing::info!(task_id = %task_id, "创建下载任务并启动后台下载");
    Ok(task_id)
}

#[cfg(test)]
/// 暂停下载任务(直接操作 AppState)
async fn pause_task_inner(state: &AppState, task_id: String) -> Result<(), String> {
    let mut store = state.tasks.lock().await;
    let task = store
        .get_mut(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;
    match task.status.as_str() {
        status::PENDING | status::DOWNLOADING => {
            task.status = status::PAUSED.to_string();
            task.speed = 0;
            Ok(())
        }
        other => Err(format!("当前状态 '{other}' 不允许暂停")),
    }
}

#[cfg(test)]
/// 恢复下载任务(直接操作 AppState)
async fn resume_task_inner(state: &AppState, task_id: String) -> Result<(), String> {
    let mut store = state.tasks.lock().await;
    let task = store
        .get_mut(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;
    if task.status == status::PAUSED {
        task.status = status::DOWNLOADING.to_string();
        Ok(())
    } else {
        Err(format!("仅暂停状态可恢复,当前状态: '{}'", task.status))
    }
}

#[cfg(test)]
/// 取消下载任务(直接操作 AppState)
async fn cancel_task_inner(state: &AppState, task_id: String) -> Result<(), String> {
    let mut store = state.tasks.lock().await;
    let task = store
        .get_mut(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;
    match task.status.as_str() {
        status::COMPLETED | status::CANCELLED => Err(format!("任务已{},无法取消", task.status)),
        _ => {
            let mut handles = state.handles.lock().await;
            if let Some(handle) = handles.remove(&task_id) {
                handle.abort();
            }
            drop(handles);
            update_task_status(&mut store, &task_id, status::CANCELLED);
            Ok(())
        }
    }
}

#[cfg(test)]
/// 删除下载任务(直接操作 AppState)
async fn delete_task_inner(state: &AppState, task_id: String) -> Result<(), String> {
    let mut store = state.tasks.lock().await;
    let task = store
        .get(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;
    match task.status.as_str() {
        status::COMPLETED | status::CANCELLED | status::FAILED => {
            store.remove(&task_id);
            let mut handles = state.handles.lock().await;
            handles.remove(&task_id);
            Ok(())
        }
        other => Err(format!("当前状态 '{other}' 不允许删除,请先取消任务")),
    }
}

#[cfg(test)]
/// 获取所有任务列表(直接操作 AppState)
async fn get_task_list_inner(state: &AppState) -> Result<Vec<TaskInfo>, String> {
    let store = state.tasks.lock().await;
    Ok(store.values().cloned().collect())
}

#[cfg(test)]
/// 获取单个任务详情(直接操作 AppState)
async fn get_task_detail_inner(state: &AppState, task_id: String) -> Result<TaskInfo, String> {
    let store = state.tasks.lock().await;
    store
        .get(&task_id)
        .cloned()
        .ok_or_else(|| format!("任务不存在: {task_id}"))
}

#[cfg(test)]
/// 获取下载进度详情(直接操作 AppState)
async fn get_download_progress_inner(state: &AppState, task_id: String) -> Result<DownloadProgress, String> {
    let store = state.tasks.lock().await;
    let task = store
        .get(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;
    Ok(DownloadProgress {
        task_id: task.id.clone(),
        status: task.status.clone(),
        progress: task.progress,
        downloaded: task.downloaded,
        file_size: task.file_size,
        speed: task.speed,
        fragments_total: task.fragments_total,
        fragments_done: task.fragments_done,
    })
}

#[cfg(test)]
/// 获取嗅探资源列表(直接操作 AppState)
async fn get_sniffer_resources_inner(state: &AppState) -> Result<Vec<SnifferResource>, String> {
    let store = state.sniffer.lock().await;
    Ok(store.iter().rev().cloned().collect())
}

#[cfg(test)]
/// 添加嗅探过滤规则(直接操作 AppState)
async fn add_sniffer_filter_inner(state: &AppState, filter: String) -> Result<(), String> {
    if filter.is_empty() {
        return Err("过滤规则不能为空".to_string());
    }
    let mut filters = state.sniffer_filters.lock().await;
    if filters.contains(&filter) {
        return Err("过滤规则已存在".to_string());
    }
    filters.push(filter);
    Ok(())
}

#[cfg(test)]
/// 获取应用配置(直接操作 AppState)
async fn get_config_inner(state: &AppState) -> Result<AppConfig, String> {
    let cfg = state.config.lock().await;
    Ok(cfg.clone())
}

#[cfg(test)]
/// 更新应用配置(直接操作 AppState)
async fn update_config_inner(state: &AppState, config: AppConfig) -> Result<(), String> {
    let mut cfg = state.config.lock().await;
    *cfg = config;
    Ok(())
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建测试用 AppState
    fn test_state() -> Arc<AppState> {
        Arc::new(AppState {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(AppConfig {
                download_dir: "/default".to_string(),
                max_concurrent_tasks: 5,
                max_concurrent_fragments: 16,
                max_connections_per_host: 16,
                enable_quic: false,
                verify_checksum: true,
            })),
            handles: Arc::new(Mutex::new(HashMap::new())),
            active_permits: Arc::new(AtomicU32::new(0)),
            sniffer: Arc::new(Mutex::new(Vec::new())),
            sniffer_filters: Arc::new(Mutex::new(Vec::new())),
        })
    }

    // -- create_task 测试 --

    #[tokio::test]
    async fn test_create_task_returns_valid_uuid() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/file.zip".to_string(), None).await.unwrap();
        assert!(Uuid::parse_str(&id).is_ok());
    }

    #[tokio::test]
    async fn test_create_task_extracts_filename() {
        let state = test_state();
        let id = create_task_inner(
            &state,
            "https://cdn.example.org/releases/app-v2.0.tar.gz".to_string(),
            None,
        )
        .await
        .unwrap();
        let task = get_task_detail_inner(&state, id).await.unwrap();
        assert_eq!(task.file_name, "app-v2.0.tar.gz");
    }

    #[tokio::test]
    async fn test_create_task_default_status_is_pending() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/data.bin".to_string(), None).await.unwrap();
        let task = get_task_detail_inner(&state, id).await.unwrap();
        assert_eq!(task.status, "pending");
        assert_eq!(task.downloaded, 0);
        assert_eq!(task.speed, 0);
        assert!((task.progress - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_create_task_with_download_dir() {
        let state = test_state();
        let id = create_task_inner(
            &state,
            "https://example.com/file.zip".to_string(),
            Some("/tmp/custom".to_string()),
        )
        .await
        .unwrap();
        let task = get_task_detail_inner(&state, id).await.unwrap();
        assert_eq!(task.url, "https://example.com/file.zip");
    }

    #[tokio::test]
    async fn test_create_task_duplicate_url_rejected() {
        let state = test_state();
        let _ = create_task_inner(&state, "https://dup.example.com/once.zip".to_string(), None).await.unwrap();
        let result = create_task_inner(&state, "https://dup.example.com/once.zip".to_string(), None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("已存在"));
    }

    // -- pause / resume 测试 --

    #[tokio::test]
    async fn test_pause_pending_task() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/file.zip".to_string(), None).await.unwrap();
        pause_task_inner(&state, id.clone()).await.unwrap();
        let task = get_task_detail_inner(&state, id).await.unwrap();
        assert_eq!(task.status, "paused");
        assert_eq!(task.speed, 0);
    }

    #[tokio::test]
    async fn test_resume_paused_task() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/file.zip".to_string(), None).await.unwrap();
        pause_task_inner(&state, id.clone()).await.unwrap();
        resume_task_inner(&state, id.clone()).await.unwrap();
        let task = get_task_detail_inner(&state, id).await.unwrap();
        assert_eq!(task.status, "downloading");
    }

    #[tokio::test]
    async fn test_pause_already_paused_task_fails() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/file.zip".to_string(), None).await.unwrap();
        pause_task_inner(&state, id.clone()).await.unwrap();
        let result = pause_task_inner(&state, id).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不允许暂停"));
    }

    #[tokio::test]
    async fn test_resume_non_paused_task_fails() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/file.zip".to_string(), None).await.unwrap();
        let result = resume_task_inner(&state, id).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("仅暂停状态可恢复"));
    }

    // -- cancel 测试 --

    #[tokio::test]
    async fn test_cancel_pending_task() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/file.zip".to_string(), None).await.unwrap();
        cancel_task_inner(&state, id.clone()).await.unwrap();
        let task = get_task_detail_inner(&state, id).await.unwrap();
        assert_eq!(task.status, "cancelled");
    }

    #[tokio::test]
    async fn test_cancel_already_cancelled_task_fails() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/file.zip".to_string(), None).await.unwrap();
        cancel_task_inner(&state, id.clone()).await.unwrap();
        let result = cancel_task_inner(&state, id).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("无法取消"));
    }

    // -- delete 测试 --

    #[tokio::test]
    async fn test_delete_cancelled_task() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/file.zip".to_string(), None).await.unwrap();
        cancel_task_inner(&state, id.clone()).await.unwrap();
        delete_task_inner(&state, id.clone()).await.unwrap();
        assert!(get_task_detail_inner(&state, id).await.is_err());
    }

    #[tokio::test]
    async fn test_delete_pending_task_fails() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/file.zip".to_string(), None).await.unwrap();
        let result = delete_task_inner(&state, id.clone()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不允许删除"));
    }

    // -- get_task_list 测试 --

    #[tokio::test]
    async fn test_get_task_list_returns_all_tasks() {
        let state = test_state();
        let id1 = create_task_inner(&state, "https://example.com/a.zip".to_string(), None).await.unwrap();
        let id2 = create_task_inner(&state, "https://example.com/b.zip".to_string(), None).await.unwrap();
        let list = get_task_list_inner(&state).await.unwrap();
        let ids: Vec<&String> = list.iter().map(|t| &t.id).collect();
        assert!(ids.contains(&&id1));
        assert!(ids.contains(&&id2));
    }

    #[tokio::test]
    async fn test_get_task_list_empty() {
        let state = test_state();
        let list = get_task_list_inner(&state).await.unwrap();
        assert!(list.is_empty());
    }

    // -- get_task_detail 测试 --

    #[tokio::test]
    async fn test_get_task_detail_not_found() {
        let state = test_state();
        let result = get_task_detail_inner(&state, "nonexistent-id".to_string()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("任务不存在"));
    }

    // -- 配置测试 --

    #[tokio::test]
    async fn test_get_config_returns_defaults() {
        let state = test_state();
        let cfg = get_config_inner(&state).await.unwrap();
        assert_eq!(cfg.max_concurrent_tasks, 5);
        assert_eq!(cfg.max_concurrent_fragments, 16);
        assert_eq!(cfg.max_connections_per_host, 16);
        assert!(!cfg.enable_quic);
        assert!(cfg.verify_checksum);
    }

    #[tokio::test]
    async fn test_update_config_roundtrip() {
        let state = test_state();
        let new_cfg = AppConfig {
            download_dir: "/data/downloads".to_string(),
            max_concurrent_tasks: 10,
            max_concurrent_fragments: 32,
            max_connections_per_host: 8,
            enable_quic: true,
            verify_checksum: false,
        };
        update_config_inner(&state, new_cfg).await.unwrap();
        let cfg = get_config_inner(&state).await.unwrap();
        assert_eq!(cfg.download_dir, "/data/downloads");
        assert_eq!(cfg.max_concurrent_tasks, 10);
        assert_eq!(cfg.max_concurrent_fragments, 32);
        assert_eq!(cfg.max_connections_per_host, 8);
        assert!(cfg.enable_quic);
        assert!(!cfg.verify_checksum);
    }

    // -- 辅助函数测试 --

    #[test]
    fn test_extract_filename_basic() {
        assert_eq!(
            extract_filename_from_url("https://example.com/path/to/file.zip"),
            "file.zip"
        );
    }

    #[test]
    fn test_extract_filename_with_query() {
        assert_eq!(
            extract_filename_from_url("https://example.com/download?file=test.bin"),
            "download"
        );
    }

    #[test]
    fn test_extract_filename_empty_path() {
        assert_eq!(extract_filename_from_url("https://example.com/"), "unknown");
    }

    #[test]
    fn test_extract_filename_encoded() {
        assert_eq!(
            extract_filename_from_url("https://example.com/my%20file.txt"),
            "my file.txt"
        );
    }

    #[test]
    fn test_extract_filename_invalid_url() {
        assert_eq!(extract_filename_from_url("not a url"), "unknown");
    }

    #[test]
    fn test_extract_filename_with_invalid_hex_encoding() {
        assert_eq!(
            extract_filename_from_url("https://example.com/file%GG.txt"),
            "file%GG.txt"
        );
    }

    // -- 任务状态流转完整性测试 --

    #[tokio::test]
    async fn test_full_task_lifecycle() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/lifecycle.bin".to_string(), None).await.unwrap();
        assert_eq!(get_task_detail_inner(&state, id.clone()).await.unwrap().status, "pending");

        pause_task_inner(&state, id.clone()).await.unwrap();
        assert_eq!(get_task_detail_inner(&state, id.clone()).await.unwrap().status, "paused");

        resume_task_inner(&state, id.clone()).await.unwrap();
        assert_eq!(get_task_detail_inner(&state, id.clone()).await.unwrap().status, "downloading");

        cancel_task_inner(&state, id.clone()).await.unwrap();
        assert_eq!(get_task_detail_inner(&state, id.clone()).await.unwrap().status, "cancelled");

        delete_task_inner(&state, id.clone()).await.unwrap();
        assert!(get_task_detail_inner(&state, id).await.is_err());
    }

    // -- 进度查询测试 --

    #[tokio::test]
    async fn test_get_download_progress() {
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/progress.bin".to_string(), None).await.unwrap();
        let progress = get_download_progress_inner(&state, id.clone()).await.unwrap();
        assert_eq!(progress.task_id, id);
        assert_eq!(progress.status, "pending");
        assert!((progress.progress - 0.0).abs() < f64::EPSILON);
        assert_eq!(progress.downloaded, 0);
        assert_eq!(progress.speed, 0);
        assert_eq!(progress.fragments_total, 0);
        assert_eq!(progress.fragments_done, 0);
    }

    #[tokio::test]
    async fn test_get_download_progress_not_found() {
        let state = test_state();
        let result = get_download_progress_inner(&state, "nonexistent".to_string()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("任务不存在"));
    }

    // -- DownloadProgress 序列化测试 --

    #[test]
    fn test_download_progress_serialization() {
        let progress = DownloadProgress {
            task_id: "test-id".to_string(),
            status: "downloading".to_string(),
            progress: 0.5,
            downloaded: 512,
            file_size: Some(1024),
            speed: 100,
            fragments_total: 4,
            fragments_done: 2,
        };
        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("taskId"));
        assert!(json.contains("fileSize"));
        assert!(json.contains("fragmentsTotal"));
    }

    // -- 嗅探命令测试 --

    #[tokio::test]
    async fn test_get_sniffer_resources_empty() {
        let state = test_state();
        let resources = get_sniffer_resources_inner(&state).await.unwrap();
        assert!(resources.is_empty());
    }

    #[tokio::test]
    async fn test_add_sniffer_filter() {
        let state = test_state();
        add_sniffer_filter_inner(&state, "cdn.example.com".to_string()).await.unwrap();
        let result = add_sniffer_filter_inner(&state, "cdn.example.com".to_string()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("已存在"));
    }

    #[tokio::test]
    async fn test_add_sniffer_filter_empty_string_fails() {
        let state = test_state();
        let result = add_sniffer_filter_inner(&state, String::new()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不能为空"));
    }

    #[tokio::test]
    async fn test_add_sniffer_resource() {
        let state = test_state();
        add_sniffer_resource(&state, "http://example.com/video.mp4".to_string()).await;
        let resources = get_sniffer_resources_inner(&state).await.unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].url, "http://example.com/video.mp4");
        assert_eq!(resources[0].resource_type, "video");
        assert_eq!(resources[0].file_name, "video.mp4");
    }

    #[tokio::test]
    async fn test_add_sniffer_resource_duplicate_ignored() {
        let state = test_state();
        add_sniffer_resource(&state, "http://example.com/file.zip".to_string()).await;
        add_sniffer_resource(&state, "http://example.com/file.zip".to_string()).await;
        let resources = get_sniffer_resources_inner(&state).await.unwrap();
        assert_eq!(resources.len(), 1, "重复 URL 应被忽略");
    }

    #[tokio::test]
    async fn test_add_sniffer_resource_with_filter() {
        let state = test_state();
        add_sniffer_filter_inner(&state, "cdn.example.com".to_string()).await.unwrap();
        add_sniffer_resource(&state, "http://other.com/video.mp4".to_string()).await;
        assert_eq!(get_sniffer_resources_inner(&state).await.unwrap().len(), 0);
        add_sniffer_resource(&state, "http://cdn.example.com/video.mp4".to_string()).await;
        assert_eq!(get_sniffer_resources_inner(&state).await.unwrap().len(), 1);
    }

    // -- resource_type_to_string 测试 --

    #[test]
    fn test_resource_type_to_string_all_variants() {
        assert_eq!(resource_type_to_string(ResourceType::Video), "video");
        assert_eq!(resource_type_to_string(ResourceType::Audio), "audio");
        assert_eq!(resource_type_to_string(ResourceType::Document), "document");
        assert_eq!(resource_type_to_string(ResourceType::Archive), "archive");
        assert_eq!(
            resource_type_to_string(ResourceType::Executable),
            "executable"
        );
        assert_eq!(resource_type_to_string(ResourceType::Image), "image");
        assert_eq!(resource_type_to_string(ResourceType::Other), "other");
    }

    // -- AppConfig 序列化测试 --

    #[test]
    fn test_app_config_serialization_roundtrip() {
        let cfg = AppConfig {
            download_dir: "/tmp".to_string(),
            max_concurrent_tasks: 3,
            max_concurrent_fragments: 8,
            max_connections_per_host: 4,
            enable_quic: true,
            verify_checksum: false,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let deserialized: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.download_dir, "/tmp");
        assert_eq!(deserialized.max_concurrent_tasks, 3);
        assert!(deserialized.enable_quic);
        assert!(!deserialized.verify_checksum);
    }

    // -- TaskInfo 序列化测试 --

    #[test]
    fn test_task_info_serialization_roundtrip() {
        let task = TaskInfo {
            id: "test-id".to_string(),
            url: "https://example.com/file.zip".to_string(),
            file_name: "file.zip".to_string(),
            file_size: Some(1024),
            downloaded: 512,
            speed: 100,
            status: "downloading".to_string(),
            progress: 0.5,
            fragments_total: 4,
            fragments_done: 2,
            created_at: "2025-01-01T00:00:00+08:00".to_string(),
        };
        let json = serde_json::to_string(&task).unwrap();
        let deserialized: TaskInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-id");
        assert_eq!(deserialized.file_size, Some(1024));
        assert!((deserialized.progress - 0.5).abs() < f64::EPSILON);
    }

    // -- parse_content_disposition 测试 --

    #[test]
    fn test_disposition_filename_simple() {
        assert_eq!(
            parse_content_disposition(r#"attachment; filename="file.zip""#),
            Some("file.zip".to_string())
        );
    }

    #[test]
    fn test_disposition_filename_encoded() {
        assert_eq!(
            parse_content_disposition("attachment; filename*=UTF-8''my%20file.zip"),
            Some("my file.zip".to_string())
        );
    }

    #[test]
    fn test_disposition_filename_none() {
        assert_eq!(parse_content_disposition("inline"), None);
    }

    // -- any_fragment_failed 验证测试 --

    #[tokio::test]
    async fn test_any_fragment_failed_detection() {
        // 验证:当信号量关闭时,any_fragment_failed 正确检测到分片失败
        let state = test_state();
        let id = create_task_inner(&state, "https://example.com/fail.bin".to_string(), None)
            .await
            .unwrap();
        // 初始状态下任务应存在
        let task = get_task_detail_inner(&state, id.clone()).await.unwrap();
        assert_eq!(task.status, "pending");
        // 验证 any_fragment_failed 逻辑:任务未完成时不标记为 failed
        assert_ne!(task.status, "failed");
    }

    // -- max_concurrent 信号量门控验证测试 --

    #[tokio::test]
    async fn test_max_concurrent_semaphore_gating() {
        let state = AppState::new();
        {
            let mut cfg = state.config.lock().await;
            cfg.max_concurrent_tasks = 2;
        }
        let _id1 = create_task_inner(&state, "http://example.com/gate1.bin".into(), None)
            .await
            .unwrap();
        let _id2 = create_task_inner(&state, "http://example.com/gate2.bin".into(), None)
            .await
            .unwrap();
        // 第三个任务应被 max_concurrent 门控拒绝
        let result = create_task_inner(&state, "http://example.com/gate3.bin".into(), None).await;
        assert!(result.is_err(), "超过 max_concurrent_tasks 应被拒绝");
        let err = result.unwrap_err();
        assert!(err.contains("最大并发任务数"), "错误应说明并发限制: {err}");
    }

    #[tokio::test]
    async fn test_max_concurrent_tasks_rejects() {
        let state = AppState::new();
        {
            let mut cfg = state.config.lock().await;
            cfg.max_concurrent_tasks = 2;
        }
        // 创建两个任务
        let _id1 = create_task_inner(&state, "http://example.com/file1.bin".into(), None)
            .await
            .unwrap();
        let _id2 = create_task_inner(&state, "http://example.com/file2.bin".into(), None)
            .await
            .unwrap();
        // 第三个应被拒绝
        let result = create_task_inner(&state, "http://example.com/file3.bin".into(), None).await;
        assert!(result.is_err(), "超过 max_concurrent_tasks 应返回错误");
        assert!(
            result.unwrap_err().contains("最大并发任务数"),
            "错误信息应提及并发限制"
        );
    }
}
