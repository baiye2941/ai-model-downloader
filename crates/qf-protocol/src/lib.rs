//! QuantumFetch 协议层:HTTP/HTTPS/QUIC/FTP
//!
//! 实现各协议的统一传输抽象:
//! - HTTP/HTTPS 客户端(基于 reqwest)
//! - QUIC 传输(基于 quinn)
//! - FTP 客户端
//! - 统一 Protocol trait

pub mod http;
pub mod quic;

pub use http::HttpClient;
