//! Tauri 命令注册

use serde::Serialize;

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
