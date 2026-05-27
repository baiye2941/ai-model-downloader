//! QuantumFetch 校验层:GPU 加速哈希与完整性校验
//!
//! 提供多种哈希校验方案:
//! - CPU 校验(blake3 / sha256)
//! - GPU 校验(wgpu compute shader, 可选)
//! - 并行校验调度

pub mod cpu;
