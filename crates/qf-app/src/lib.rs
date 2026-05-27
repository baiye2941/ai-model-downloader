//! QuantumFetch Tauri 应用库

pub mod commands;

use commands::{get_app_info, supported_protocols};

/// 构建 Tauri 应用
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_app_info, supported_protocols])
        .run(tauri::generate_context!())
        .expect("启动 QuantumFetch 应用失败");
}
