//! QuantumFetch 嗅探层:浏览器资源拦截与解析
//!
//! 通过 Playwright MCP 拦截浏览器流量:
//! - 请求拦截与资源类型识别
//! - 下载链接提取
//! - 媒体资源捕获

pub mod capture;
pub mod filter;
