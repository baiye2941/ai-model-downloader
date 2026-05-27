//! Tauri 命令模块
//!
//! 提供应用信息查询、下载任务管理、配置管理、嗅探等 Tauri 命令。
//! 任务存储使用全局 `Mutex<HashMap>` 实现,线程安全。
//! 下载任务通过后台 tokio task 异步执行,不阻塞 Tauri 命令返回。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;
use qf_core::types::FileMetadata;
use qf_engine::DownloadOrchestrator;
use qf_engine::connection::PoolConfig;
use qf_sniffer::capture::{ResourceType, identify_resource};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

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
// 全局存储
// ---------------------------------------------------------------------------

/// 任务列表全局存储
static TASK_STORE: LazyLock<Mutex<HashMap<String, TaskInfo>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 应用配置全局存储
static CONFIG_STORE: LazyLock<Mutex<AppConfig>> = LazyLock::new(|| {
    Mutex::new(AppConfig {
        download_dir: dirs()
            .map(|p| p.join("Downloads").to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string()),
        max_concurrent_tasks: 5,
        max_concurrent_fragments: 16,
        max_connections_per_host: 16,
        enable_quic: false,
        verify_checksum: true,
    })
});

/// 后台下载任务句柄(用于取消)
static TASK_HANDLE_STORE: LazyLock<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 活跃分片许可计数(用于限制并发)
static ACTIVE_PERMITS: LazyLock<AtomicU32> = LazyLock::new(|| AtomicU32::new(0));

/// 嗅探到的资源列表
static SNIFFER_STORE: LazyLock<Mutex<Vec<SnifferResource>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// 嗅探过滤规则(URL 关键词)
static SNIFFER_FILTERS: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 获取用户主目录(Windows: USERPROFILE, Unix: HOME)
fn dirs() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
}

/// 从 URL 中提取文件名,提取失败时返回 `"unknown"`
fn extract_filename(url_str: &str) -> String {
    url::Url::parse(url_str)
        .ok()
        .and_then(|u| {
            let path = u.path();
            let segment = path.rsplit('/').next().unwrap_or("");
            if segment.is_empty() {
                None
            } else {
                // URL 解码百分号编码
                percent_decode(segment)
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// 简易百分号解码(不依赖外部 crate)
///
/// 对无效的 `%XX` 序列(如 `%GG`),保留原样字符而非返回 None。
/// 仅当最终结果不是合法 UTF-8 时返回 None。
fn percent_decode(input: &str) -> Option<String> {
    let mut output = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = parse_hex_pair(bytes[i + 1], bytes[i + 2]) {
                output.push(byte);
                i += 3;
            } else {
                // 无效百分号编码,保留 `%` 原样
                output.push(bytes[i]);
                i += 1;
            }
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(output).ok()
}

/// 解析两个十六进制 ASCII 字节为 `u8`,无效时返回 `None`
fn parse_hex_pair(high: u8, low: u8) -> Option<u8> {
    let h = hex_digit(high)?;
    let l = hex_digit(low)?;
    Some(h * 16 + l)
}

/// 单个十六进制 ASCII 字节转 `u8` (0-15),无效时返回 `None`
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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

/// 更新任务状态(需要调用方已持有 TASK_STORE 写锁或传入可变引用)
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
/// 通过检查 `TASK_STORE` 中的状态来响应暂停和取消操作。
async fn task_fn(
    task_id: String,
    url: String,
    download_dir: String,
    mut orchestrator: DownloadOrchestrator,
    metadata: FileMetadata,
) {
    // 解析 URL
    let download_url = match Url::parse(&url) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(task_id = %task_id, error = %e, "URL 解析失败");
            let mut store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
            update_task_status(&mut store, &task_id, status::FAILED);
            return;
        }
    };

    // 获取主机名
    let host = match download_url.host_str() {
        Some(h) => h.to_string(),
        None => {
            tracing::error!(task_id = %task_id, "URL 主机为空");
            let mut store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
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

    // 使用编排器计算分片策略
    let fragments = orchestrator.plan_fragments(file_size, supports_range);
    let fragment_count = fragments.len() as u32;
    tracing::info!(
        task_id = %task_id,
        fragments = fragment_count,
        supports_range = supports_range,
        "分片策略规划完成"
    );

    // 更新任务信息:设置文件大小和分片总数,标记为下载中
    {
        let mut store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = store.get_mut(&task_id) {
            task.file_size = metadata.file_size;
            task.fragments_total = fragment_count;
        }
        update_task_status(&mut store, &task_id, status::DOWNLOADING);
    }

    // 并发分片下载模拟
    let max_concurrent = {
        let cfg = CONFIG_STORE.lock().unwrap_or_else(|e| e.into_inner());
        cfg.max_concurrent_fragments as usize
    };
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrent));

    let mut any_fragment_failed = false; // 保留 mut:接入真实协议层后将在此赋值
    let mut total_downloaded: u64 = 0;

    for frag in &fragments {
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .unwrap_or_else(|_| panic!("分片信号量已关闭"));
        ACTIVE_PERMITS.fetch_add(1, Ordering::Relaxed);
        orchestrator.register_fragment(frag.clone());

        let chunk_size: u64 = 1024 * 100; // 100KB 模拟块
        let chunks = if chunk_size > 0 {
            frag.size.div_ceil(chunk_size)
        } else {
            1
        };
        let frag_start = Instant::now();
        let mut frag_downloaded: u64 = 0;

        for _ in 0..chunks {
            // 检查任务状态(响应暂停/取消)
            {
                let store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(task) = store.get(&task_id) {
                    match task.status.as_str() {
                        status::CANCELLED => {
                            tracing::info!(task_id = %task_id, "任务已取消,退出后台下载");
                            drop(store);
                            drop(permit);
                            ACTIVE_PERMITS.fetch_sub(1, Ordering::Relaxed);
                            return;
                        }
                        status::PAUSED => {
                            tracing::info!(task_id = %task_id, "任务已暂停,等待恢复...");
                        }
                        _ => {}
                    }
                }
            }

            // 暂停状态时自旋等待恢复(每 200ms 检查一次)
            {
                let mut paused_iterations = 0u32;
                loop {
                    let is_paused = TASK_STORE
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&task_id)
                        .is_some_and(|t| t.status == status::PAUSED);
                    if !is_paused {
                        break;
                    }
                    paused_iterations += 1;
                    if paused_iterations > 1500 {
                        // 超过 5 分钟暂停,标记超时失败
                        tracing::warn!(task_id = %task_id, "暂停超时,标记任务失败");
                        let mut store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
                        update_task_status(&mut store, &task_id, status::FAILED);
                        drop(permit);
                        ACTIVE_PERMITS.fetch_sub(1, Ordering::Relaxed);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }

            // 模拟网络 I/O 延迟(未来接入真实协议层时,此处替换为实际下载调用)
            tokio::time::sleep(Duration::from_millis(2)).await;
            let simulated = chunk_size.min(frag.size - frag_downloaded);
            frag_downloaded += simulated;
            total_downloaded += simulated;

            // 计算并更新速度和进度
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

            let mut store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(task) = store.get_mut(&task_id) {
                task.downloaded = total_downloaded;
                task.speed = speed;
                task.progress = progress.min(1.0);
            }
        }

        // 分片完成
        orchestrator.on_fragment_complete(frag, frag_start.elapsed());
        // TODO: 接入真实协议层后,下载失败时应设置 any_fragment_failed = true
        let fragments_done = orchestrator
            .active_fragments()
            .iter()
            .filter(|r| r.completed)
            .count() as u32;
        drop(permit);
        ACTIVE_PERMITS.fetch_sub(1, Ordering::Relaxed);

        // 更新已完成分片数
        {
            let mut store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
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

    // 所有分片处理完毕,根据是否有失败分片决定最终状态
    {
        let mut store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
        if any_fragment_failed {
            update_task_status(&mut store, &task_id, status::FAILED);
            tracing::error!(task_id = %task_id, "部分分片下载失败");
        } else {
            // 设置进度为 100%
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
pub fn create_task(url: String, download_dir: Option<String>) -> Result<String, String> {
    let task_id = Uuid::new_v4().to_string();
    let file_name = extract_filename(&url);
    let created_at = now_iso8601();

    // 检查 URL 是否已存在(防重复)
    {
        let store = TASK_STORE
            .lock()
            .map_err(|e| format!("获取任务锁失败: {e}"))?;
        if store.values().any(|t| {
            t.url == url
                && t.status != status::CANCELLED
                && t.status != status::COMPLETED
                && t.status != status::FAILED
        }) {
            return Err("相同 URL 的下载任务已存在".to_string());
        }
    }

    // 读取配置(目录等)
    let download_dir_str = {
        let cfg = CONFIG_STORE
            .lock()
            .map_err(|e| format!("获取配置锁失败: {e}"))?;
        download_dir.unwrap_or_else(|| cfg.download_dir.clone())
    };

    // 创建 TaskInfo 记录
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
        let mut store = TASK_STORE
            .lock()
            .map_err(|e| format!("获取任务锁失败: {e}"))?;
        store.insert(task_id.clone(), task);
    }

    // 创建编排器(管理分片策略、连接池、带宽追踪)
    let orchestrator = DownloadOrchestrator::new(PoolConfig {
        max_per_host: 16,
        max_global: 256,
    });

    // 先探测文件元数据,然后启动后台下载任务
    let tid = task_id.clone();
    let url_clone = url.clone();
    let handle = tokio::spawn(async move {
        // 步骤一:探测文件元数据
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
                let mut store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
                update_task_status(&mut store, &tid, status::FAILED);
                return;
            }
        };

        // 步骤二:执行分片下载
        task_fn(tid, url_clone, download_dir_str, orchestrator, metadata).await;
    });

    // 保存后台任务句柄(用于取消)
    {
        let mut handles = TASK_HANDLE_STORE
            .lock()
            .map_err(|e| format!("获取句柄锁失败: {e}"))?;
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
        .and_then(disposition_filename)
        .unwrap_or_else(|| extract_filename(url));

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

/// 从 Content-Disposition 头解析文件名
fn disposition_filename(header: &str) -> Option<String> {
    // 尝试 filename*=UTF-8''encoded_name
    if let Some(pos) = header.find("filename*=") {
        let rest = &header[pos + 10..];
        if let Some(encoded) = rest.split(';').next() {
            // 格式: UTF-8''percent_encoded
            let parts: Vec<&str> = encoded.splitn(3, '\'').collect();
            if parts.len() == 3 {
                return percent_decode(parts[2]);
            }
        }
    }
    // 尝试 filename="name"
    if let Some(pos) = header.find("filename=") {
        let rest = &header[pos + 9..];
        let name = rest.trim_start().split(';').next().unwrap_or(rest);
        let name = name.trim_matches('"').trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// 暂停下载任务
///
/// 仅 `pending` 或 `downloading` 状态的任务可以暂停。
/// 后台任务检测到暂停状态后将自旋等待恢复。
#[tauri::command]
pub fn pause_task(task_id: String) -> Result<(), String> {
    let mut store = TASK_STORE
        .lock()
        .map_err(|e| format!("获取任务锁失败: {e}"))?;

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
pub fn resume_task(task_id: String) -> Result<(), String> {
    let mut store = TASK_STORE
        .lock()
        .map_err(|e| format!("获取任务锁失败: {e}"))?;

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
pub fn cancel_task(task_id: String) -> Result<(), String> {
    let mut store = TASK_STORE
        .lock()
        .map_err(|e| format!("获取任务锁失败: {e}"))?;

    let task = store
        .get_mut(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;

    match task.status.as_str() {
        status::COMPLETED | status::CANCELLED => Err(format!("任务已{},无法取消", task.status)),
        _ => {
            // 中止后台任务
            let mut handles = TASK_HANDLE_STORE.lock().unwrap_or_else(|e| e.into_inner());
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
pub fn delete_task(task_id: String) -> Result<(), String> {
    let mut store = TASK_STORE
        .lock()
        .map_err(|e| format!("获取任务锁失败: {e}"))?;

    let task = store
        .get(&task_id)
        .ok_or_else(|| format!("任务不存在: {task_id}"))?;

    match task.status.as_str() {
        status::COMPLETED | status::CANCELLED | status::FAILED => {
            store.remove(&task_id);
            // 清理后台任务句柄
            let mut handles = TASK_HANDLE_STORE.lock().unwrap_or_else(|e| e.into_inner());
            handles.remove(&task_id);
            tracing::info!(task_id = %task_id, "删除任务");
            Ok(())
        }
        other => Err(format!("当前状态 '{other}' 不允许删除,请先取消任务")),
    }
}

/// 获取所有任务列表
#[tauri::command]
pub fn get_task_list() -> Result<Vec<TaskInfo>, String> {
    TASK_STORE
        .lock()
        .map(|store| store.values().cloned().collect())
        .map_err(|e| format!("获取任务锁失败: {e}"))
}

/// 获取单个任务详情
#[tauri::command]
pub fn get_task_detail(task_id: String) -> Result<TaskInfo, String> {
    let store = TASK_STORE
        .lock()
        .map_err(|e| format!("获取任务锁失败: {e}"))?;

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
pub fn get_download_progress(task_id: String) -> Result<DownloadProgress, String> {
    let store = TASK_STORE
        .lock()
        .map_err(|e| format!("获取任务锁失败: {e}"))?;

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
pub fn get_sniffer_resources() -> Result<Vec<SnifferResource>, String> {
    let store = SNIFFER_STORE
        .lock()
        .map_err(|e| format!("获取嗅探锁失败: {e}"))?;
    // 最新的资源排在前面
    Ok(store.iter().rev().cloned().collect())
}

/// 添加嗅探过滤规则
///
/// `filter` 为 URL 关键词,仅包含匹配关键词的资源会被捕获。
/// 规则持久化到 `SNIFFER_FILTERS`,供嗅探引擎使用。
#[tauri::command]
pub fn add_sniffer_filter(filter: String) -> Result<(), String> {
    if filter.is_empty() {
        return Err("过滤规则不能为空".to_string());
    }
    let mut filters = SNIFFER_FILTERS
        .lock()
        .map_err(|e| format!("获取过滤规则锁失败: {e}"))?;
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
pub fn add_sniffer_resource(url: String) {
    // 检查过滤规则
    let filters = SNIFFER_FILTERS.lock().unwrap_or_else(|e| e.into_inner());
    if !filters.is_empty() && !filters.iter().any(|f| url.contains(f.as_str())) {
        return;
    }
    drop(filters);

    let resource_type = identify_resource(&url);
    let file_name = extract_filename(&url);
    let resource = SnifferResource {
        id: Uuid::new_v4().to_string(),
        url: url.clone(),
        resource_type: resource_type_to_string(resource_type).to_string(),
        file_name,
        captured_at: now_iso8601(),
    };

    let mut store = SNIFFER_STORE.lock().unwrap_or_else(|e| e.into_inner());

    // 避免重复添加
    if store.iter().any(|r| r.url == url) {
        return;
    }

    // 限制最大存储数量,避免内存无限增长
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
pub fn get_config() -> Result<AppConfig, String> {
    CONFIG_STORE
        .lock()
        .map(|cfg| cfg.clone())
        .map_err(|e| format!("获取配置锁失败: {e}"))
}

/// 更新应用配置
///
/// 前端传入完整的新配置,整体替换旧配置。
#[tauri::command]
pub fn update_config(config: AppConfig) -> Result<(), String> {
    let mut cfg = CONFIG_STORE
        .lock()
        .map_err(|e| format!("获取配置锁失败: {e}"))?;
    *cfg = config;
    tracing::info!("应用配置已更新");
    Ok(())
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// 重置全局配置为默认值(含中毒恢复)
    fn reset_config() {
        let mut cfg = CONFIG_STORE.lock().unwrap_or_else(|e| e.into_inner());
        *cfg = AppConfig {
            download_dir: "/default".to_string(),
            max_concurrent_tasks: 5,
            max_concurrent_fragments: 16,
            max_connections_per_host: 16,
            enable_quic: false,
            verify_checksum: true,
        };
    }

    /// 清空全局任务存储(含中毒恢复)
    fn clear_tasks() {
        let mut store = TASK_STORE.lock().unwrap_or_else(|e| e.into_inner());
        store.clear();
        let mut handles = TASK_HANDLE_STORE.lock().unwrap_or_else(|e| e.into_inner());
        handles.clear();
    }

    /// 清空嗅探存储
    fn clear_sniffer() {
        let mut store = SNIFFER_STORE.lock().unwrap_or_else(|e| e.into_inner());
        store.clear();
        let mut filters = SNIFFER_FILTERS.lock().unwrap_or_else(|e| e.into_inner());
        filters.clear();
    }

    // -- create_task 测试 --

    #[serial]
    #[tokio::test]
    async fn test_create_task_returns_valid_uuid() {
        clear_tasks();
        let id = create_task("https://example.com/file.zip".to_string(), None).unwrap();
        // 验证返回值是合法 UUID
        assert!(Uuid::parse_str(&id).is_ok());
    }

    #[serial]
    #[tokio::test]
    async fn test_create_task_extracts_filename() {
        clear_tasks();
        let id = create_task(
            "https://cdn.example.org/releases/app-v2.0.tar.gz".to_string(),
            None,
        )
        .unwrap();
        let task = get_task_detail(id).unwrap();
        assert_eq!(task.file_name, "app-v2.0.tar.gz");
    }

    #[serial]
    #[tokio::test]
    async fn test_create_task_default_status_is_pending() {
        clear_tasks();
        let id = create_task("https://example.com/data.bin".to_string(), None).unwrap();
        let task = get_task_detail(id).unwrap();
        assert_eq!(task.status, "pending");
        assert_eq!(task.downloaded, 0);
        assert_eq!(task.speed, 0);
        assert!((task.progress - 0.0).abs() < f64::EPSILON);
    }

    #[serial]
    #[tokio::test]
    async fn test_create_task_with_download_dir() {
        clear_tasks();
        reset_config();
        let id = create_task(
            "https://example.com/file.zip".to_string(),
            Some("/tmp/custom".to_string()),
        )
        .unwrap();
        let task = get_task_detail(id).unwrap();
        // 验证任务已创建
        assert_eq!(task.url, "https://example.com/file.zip");
    }

    #[serial]
    #[tokio::test]
    async fn test_create_task_duplicate_url_rejected() {
        clear_tasks();
        let _ = create_task("https://dup.example.com/once.zip".to_string(), None).unwrap();
        let result = create_task("https://dup.example.com/once.zip".to_string(), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("已存在"));
    }

    // -- pause / resume 测试 --

    #[serial]
    #[tokio::test]
    async fn test_pause_pending_task() {
        clear_tasks();
        let id = create_task("https://example.com/file.zip".to_string(), None).unwrap();
        pause_task(id.clone()).unwrap();
        let task = get_task_detail(id).unwrap();
        assert_eq!(task.status, "paused");
        assert_eq!(task.speed, 0);
    }

    #[serial]
    #[tokio::test]
    async fn test_resume_paused_task() {
        clear_tasks();
        let id = create_task("https://example.com/file.zip".to_string(), None).unwrap();
        pause_task(id.clone()).unwrap();
        resume_task(id.clone()).unwrap();
        let task = get_task_detail(id).unwrap();
        assert_eq!(task.status, "downloading");
    }

    #[serial]
    #[tokio::test]
    async fn test_pause_already_paused_task_fails() {
        clear_tasks();
        let id = create_task("https://example.com/file.zip".to_string(), None).unwrap();
        pause_task(id.clone()).unwrap();
        let result = pause_task(id);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不允许暂停"));
    }

    #[serial]
    #[tokio::test]
    async fn test_resume_non_paused_task_fails() {
        clear_tasks();
        let id = create_task("https://example.com/file.zip".to_string(), None).unwrap();
        // pending 状态不可恢复
        let result = resume_task(id);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("仅暂停状态可恢复"));
    }

    // -- cancel 测试 --

    #[serial]
    #[tokio::test]
    async fn test_cancel_pending_task() {
        clear_tasks();
        let id = create_task("https://example.com/file.zip".to_string(), None).unwrap();
        cancel_task(id.clone()).unwrap();
        let task = get_task_detail(id).unwrap();
        assert_eq!(task.status, "cancelled");
    }

    #[serial]
    #[tokio::test]
    async fn test_cancel_already_cancelled_task_fails() {
        clear_tasks();
        let id = create_task("https://example.com/file.zip".to_string(), None).unwrap();
        cancel_task(id.clone()).unwrap();
        let result = cancel_task(id);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("无法取消"));
    }

    // -- delete 测试 --

    #[serial]
    #[tokio::test]
    async fn test_delete_cancelled_task() {
        clear_tasks();
        let id = create_task("https://example.com/file.zip".to_string(), None).unwrap();
        cancel_task(id.clone()).unwrap();
        delete_task(id.clone()).unwrap();
        assert!(get_task_detail(id).is_err());
    }

    #[serial]
    #[tokio::test]
    async fn test_delete_pending_task_fails() {
        clear_tasks();
        let id = create_task("https://example.com/file.zip".to_string(), None).unwrap();
        let result = delete_task(id.clone());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不允许删除"));
    }

    // -- get_task_list 测试 --

    #[serial]
    #[tokio::test]
    async fn test_get_task_list_returns_all_tasks() {
        clear_tasks();
        let id1 = create_task("https://example.com/a.zip".to_string(), None).unwrap();
        let id2 = create_task("https://example.com/b.zip".to_string(), None).unwrap();
        let list = get_task_list().unwrap();
        let ids: Vec<&String> = list.iter().map(|t| &t.id).collect();
        assert!(ids.contains(&&id1));
        assert!(ids.contains(&&id2));
    }

    #[serial]
    #[test]
    fn test_get_task_list_empty() {
        clear_tasks();
        let list = get_task_list().unwrap();
        assert!(list.is_empty());
    }

    // -- get_task_detail 测试 --

    #[serial]
    #[test]
    fn test_get_task_detail_not_found() {
        clear_tasks();
        let result = get_task_detail("nonexistent-id".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("任务不存在"));
    }

    // -- 配置测试 --

    #[serial]
    #[test]
    fn test_get_config_returns_defaults() {
        reset_config();
        let cfg = get_config().unwrap();
        assert_eq!(cfg.max_concurrent_tasks, 5);
        assert_eq!(cfg.max_concurrent_fragments, 16);
        assert_eq!(cfg.max_connections_per_host, 16);
        assert!(!cfg.enable_quic);
        assert!(cfg.verify_checksum);
    }

    #[serial]
    #[test]
    fn test_update_config_roundtrip() {
        // 保存旧配置并在测试结束后恢复,避免污染其他测试
        let old_cfg = get_config().unwrap();
        let new_cfg = AppConfig {
            download_dir: "/data/downloads".to_string(),
            max_concurrent_tasks: 10,
            max_concurrent_fragments: 32,
            max_connections_per_host: 8,
            enable_quic: true,
            verify_checksum: false,
        };
        update_config(new_cfg).unwrap();
        let cfg = get_config().unwrap();
        assert_eq!(cfg.download_dir, "/data/downloads");
        assert_eq!(cfg.max_concurrent_tasks, 10);
        assert_eq!(cfg.max_concurrent_fragments, 32);
        assert_eq!(cfg.max_connections_per_host, 8);
        assert!(cfg.enable_quic);
        assert!(!cfg.verify_checksum);
        // 恢复旧配置
        let _ = update_config(old_cfg);
    }

    // -- 辅助函数测试 --

    #[serial]
    #[test]
    fn test_extract_filename_basic() {
        assert_eq!(
            extract_filename("https://example.com/path/to/file.zip"),
            "file.zip"
        );
    }

    #[serial]
    #[test]
    fn test_extract_filename_with_query() {
        assert_eq!(
            extract_filename("https://example.com/download?file=test.bin"),
            "download"
        );
    }

    #[serial]
    #[test]
    fn test_extract_filename_empty_path() {
        assert_eq!(extract_filename("https://example.com/"), "unknown");
    }

    #[serial]
    #[test]
    fn test_extract_filename_encoded() {
        assert_eq!(
            extract_filename("https://example.com/my%20file.txt"),
            "my file.txt"
        );
    }

    #[serial]
    #[test]
    fn test_extract_filename_invalid_url() {
        assert_eq!(extract_filename("not a url"), "unknown");
    }

    #[serial]
    #[test]
    fn test_extract_filename_with_invalid_hex_encoding() {
        // 无效百分号编码应保留原样字符而非回退到 "unknown"
        assert_eq!(
            extract_filename("https://example.com/file%GG.txt"),
            "file%GG.txt"
        );
    }

    #[serial]
    #[test]
    fn test_percent_decode_basic() {
        assert_eq!(
            percent_decode("hello%20world"),
            Some("hello world".to_string())
        );
    }

    #[serial]
    #[test]
    fn test_percent_decode_no_encoding() {
        assert_eq!(
            percent_decode("filename.zip"),
            Some("filename.zip".to_string())
        );
    }

    #[serial]
    #[test]
    fn test_percent_decode_invalid_hex_preserves_literal() {
        // 无效的 %GG 应保留原样,而非返回 None
        assert_eq!(
            percent_decode("file%GG.txt"),
            Some("file%GG.txt".to_string())
        );
    }

    // -- 任务状态流转完整性测试 --

    #[serial]
    #[tokio::test]
    async fn test_full_task_lifecycle() {
        clear_tasks();
        // 创建 -> 暂停 -> 恢复 -> 取消 -> 删除
        let id = create_task("https://example.com/lifecycle.bin".to_string(), None).unwrap();
        assert_eq!(get_task_detail(id.clone()).unwrap().status, "pending");

        pause_task(id.clone()).unwrap();
        assert_eq!(get_task_detail(id.clone()).unwrap().status, "paused");

        resume_task(id.clone()).unwrap();
        assert_eq!(get_task_detail(id.clone()).unwrap().status, "downloading");

        cancel_task(id.clone()).unwrap();
        assert_eq!(get_task_detail(id.clone()).unwrap().status, "cancelled");

        delete_task(id.clone()).unwrap();
        assert!(get_task_detail(id).is_err());
    }

    // -- 进度查询测试 --

    #[serial]
    #[tokio::test]
    async fn test_get_download_progress() {
        clear_tasks();
        let id = create_task("https://example.com/progress.bin".to_string(), None).unwrap();
        let progress = get_download_progress(id.clone()).unwrap();
        assert_eq!(progress.task_id, id);
        assert_eq!(progress.status, "pending");
        assert!((progress.progress - 0.0).abs() < f64::EPSILON);
        assert_eq!(progress.downloaded, 0);
        assert_eq!(progress.speed, 0);
        assert_eq!(progress.fragments_total, 0);
        assert_eq!(progress.fragments_done, 0);
    }

    #[serial]
    #[test]
    fn test_get_download_progress_not_found() {
        clear_tasks();
        let result = get_download_progress("nonexistent".to_string());
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

    #[serial]
    #[test]
    fn test_get_sniffer_resources_empty() {
        clear_sniffer();
        let resources = get_sniffer_resources().unwrap();
        assert!(resources.is_empty());
    }

    #[serial]
    #[test]
    fn test_add_sniffer_filter() {
        clear_sniffer();
        add_sniffer_filter("cdn.example.com".to_string()).unwrap();
        // 添加重复规则应失败
        let result = add_sniffer_filter("cdn.example.com".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("已存在"));
    }

    #[serial]
    #[test]
    fn test_add_sniffer_filter_empty_string_fails() {
        clear_sniffer();
        let result = add_sniffer_filter(String::new());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不能为空"));
    }

    #[serial]
    #[test]
    fn test_add_sniffer_resource() {
        clear_sniffer();
        add_sniffer_resource("http://example.com/video.mp4".to_string());
        let resources = get_sniffer_resources().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].url, "http://example.com/video.mp4");
        assert_eq!(resources[0].resource_type, "video");
        assert_eq!(resources[0].file_name, "video.mp4");
    }

    #[serial]
    #[test]
    fn test_add_sniffer_resource_duplicate_ignored() {
        clear_sniffer();
        add_sniffer_resource("http://example.com/file.zip".to_string());
        add_sniffer_resource("http://example.com/file.zip".to_string());
        let resources = get_sniffer_resources().unwrap();
        assert_eq!(resources.len(), 1, "重复 URL 应被忽略");
    }

    #[serial]
    #[test]
    fn test_add_sniffer_resource_with_filter() {
        clear_sniffer();
        add_sniffer_filter("cdn.example.com".to_string()).unwrap();
        // 不匹配过滤规则的 URL 应被忽略
        add_sniffer_resource("http://other.com/video.mp4".to_string());
        assert_eq!(get_sniffer_resources().unwrap().len(), 0);
        // 匹配的 URL 应被捕获
        add_sniffer_resource("http://cdn.example.com/video.mp4".to_string());
        assert_eq!(get_sniffer_resources().unwrap().len(), 1);
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

    #[serial]
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

    #[serial]
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

    // -- disposition_filename 测试 --

    #[test]
    fn test_disposition_filename_simple() {
        assert_eq!(
            disposition_filename(r#"attachment; filename="file.zip""#),
            Some("file.zip".to_string())
        );
    }

    #[test]
    fn test_disposition_filename_encoded() {
        assert_eq!(
            disposition_filename("attachment; filename*=UTF-8''my%20file.zip"),
            Some("my file.zip".to_string())
        );
    }

    #[test]
    fn test_disposition_filename_none() {
        assert_eq!(disposition_filename("inline"), None);
    }
}
