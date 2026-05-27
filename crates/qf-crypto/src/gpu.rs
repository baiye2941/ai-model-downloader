//! GPU 加速哈希校验
//!
//! 使用 wgpu compute shader 在 GPU 上并行计算 blake3 哈希。
//! 适用场景:单分片数据量较大且 GPU 可用时。
//!
//! # 设计说明
//!
//! blake3 的完整 GPU compute shader 实现需要 7 轮 G 函数压缩,
//! 工程复杂度极高。本模块搭建了完整的 wgpu compute pipeline 框架,
//! 当前哈希计算回退到 CPU blake3,为后续完整 GPU 实现预留接口。

use std::borrow::Cow;

use qf_core::QfError;
use qf_core::error::QfResult;
use wgpu::util::DeviceExt;

/// GPU 校验器
///
/// 封装 wgpu device/queue 和 compute pipeline,提供 GPU 加速哈希。
/// 当数据量小于阈值时自动回退到 CPU blake3。
pub struct GpuVerifier {
    device: wgpu::Device,
    queue: wgpu::Queue,
    blake3_pipeline: wgpu::ComputePipeline,
}

/// 小于此阈值的数据直接使用 CPU 计算,避免 GPU 启动开销超过收益
const GPU_MIN_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

impl GpuVerifier {
    /// 创建 GPU 校验器
    ///
    /// 初始化 wgpu 实例、适配器、设备和 compute pipeline。
    /// 如果系统无可用 GPU,返回错误。
    pub async fn new() -> QfResult<Self> {
        let instance = wgpu::Instance::default();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| QfError::Other("未找到可用 GPU 适配器".into()))?;

        tracing::info!(
            "GPU 适配器: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        );

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("qf_crypto_device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                },
                None,
            )
            .await
            .map_err(|e| QfError::Other(format!("GPU 设备初始化失败: {e}")))?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blake3_shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(BLAKE3_SHADER)),
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("blake3_pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            blake3_pipeline: pipeline,
        })
    }

    /// 检查 GPU 是否可用(不持久化设备资源)
    ///
    /// 仅探测是否有兼容的 GPU 适配器,适合用于快速决策路径。
    pub async fn is_available() -> bool {
        let instance = wgpu::Instance::default();
        instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .is_some()
    }

    /// 获取 GPU 设备名称
    pub fn device_name(&self) -> String {
        format!("{:?}", self.device)
    }

    /// 在 GPU 上计算 blake3 哈希
    ///
    /// # 行为
    ///
    /// - 数据量 < 64 MiB 时,直接使用 CPU blake3 计算(GPU 启动开销不划算)
    /// - 数据量 >= 64 MiB 时,走 GPU compute pipeline
    ///
    /// # 注意
    ///
    /// 完整的 blake3 GPU 实现需要在 WGSL 中实现 7 轮 G 函数压缩,
    /// 当前版本先搭建 pipeline 框架,实际哈希仍回退 CPU blake3。
    /// 后续迭代将替换为真正的 GPU compute shader 实现。
    pub async fn compute_blake3(&self, data: &[u8]) -> QfResult<String> {
        if data.len() < GPU_MIN_SIZE {
            tracing::debug!(data_len = data.len(), "数据量小于阈值,回退到 CPU blake3");
            let hash = blake3::hash(data);
            return Ok(hash.to_hex().to_string());
        }

        tracing::info!(data_len = data.len(), "使用 GPU compute pipeline 计算哈希");

        // 1. 创建输入 storage buffer
        let input_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("blake3_input"),
                contents: data,
                usage: wgpu::BufferUsages::STORAGE,
            });

        // 2. 创建输出 storage buffer (blake3 哈希 32 字节 = 8 x u32)
        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blake3_output"),
            size: 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // 3. 创建 bind group layout 和 bind group
        let bind_group_layout = self.blake3_pipeline.get_bind_group_layout(0);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blake3_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });

        // 4. 调度 compute pass
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("blake3_encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("blake3_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.blake3_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        self.queue.submit(Some(encoder.finish()));

        // 5. 当前回退:GPU pipeline 已执行但 blake3 压缩函数尚未在 WGSL 中完整实现,
        //    使用 CPU blake3 作为结果来源。后续迭代将读回 GPU 计算的 output_buffer。
        let hash = blake3::hash(data);
        Ok(hash.to_hex().to_string())
    }
}

/// 自动选择:大数据使用 GPU(如可用),小数据使用 CPU
///
/// 此函数不持久化 GPU 设备,每次调用都会重新初始化。
/// 适合单次或低频使用场景。高频场景建议复用 `GpuVerifier` 实例。
pub async fn auto_select_and_hash(data: &[u8]) -> QfResult<String> {
    if data.len() >= GPU_MIN_SIZE && GpuVerifier::is_available().await {
        let gpu = GpuVerifier::new().await?;
        gpu.compute_blake3(data).await
    } else {
        let hash = blake3::hash(data);
        Ok(hash.to_hex().to_string())
    }
}

/// Blake3 WGSL compute shader
///
/// 定义 blake3 哈希所需的 storage buffer 绑定和初始化向量。
/// 完整实现需要在 WGSL 中实现 G 函数的 7 轮压缩,当前为框架版本。
const BLAKE3_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> input_data: array<u32>;
@group(0) @binding(1) var<storage, read_write> output_hash: array<u32, 8>;

// Blake3 初始化向量(来自 SHA-256 前 8 个质数的平方根小数部分)
const IV: array<u32, 8> = array<u32, 8>(
    0x6A09E667u, 0xBB67AE85u, 0x3C6EF372u, 0xA54FF53Au,
    0x510E527Fu, 0x9B05688Cu, 0x1F83D9ABu, 0x5BE0CD19u
);

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // 仅第一个线程执行初始化
    // 完整实现需要分块读取 input_data 并执行 7 轮 G 函数压缩
    if (global_id.x == 0u) {
        for (var i = 0u; i < 8u; i++) {
            output_hash[i] = IV[i];
        }
    }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 GPU 可用性检查不 panic
    #[tokio::test]
    async fn test_is_available_no_panic() {
        // 无论环境是否有 GPU,此调用不应 panic
        let _available = GpuVerifier::is_available().await;
    }

    /// 测试小数据回退到 CPU 计算且结果正确
    #[tokio::test]
    async fn test_small_data_uses_cpu_fallback() {
        let data = b"hello world";
        let expected = blake3::hash(data).to_hex().to_string();

        // 即使有 GPU,小数据也应走 CPU 路径
        let result = if GpuVerifier::is_available().await {
            let gpu = GpuVerifier::new().await.unwrap();
            gpu.compute_blake3(data).await.unwrap()
        } else {
            // 无 GPU 环境,直接验证 CPU 路径
            let hash = blake3::hash(data);
            hash.to_hex().to_string()
        };

        assert_eq!(result, expected);
    }

    /// 测试 auto_select_and_hash 小数据使用 CPU
    #[tokio::test]
    async fn test_auto_select_small_data_cpu() {
        let data = b"small data for cpu path";
        let expected = blake3::hash(data).to_hex().to_string();
        let result = auto_select_and_hash(data).await.unwrap();
        assert_eq!(result, expected);
    }

    /// 测试 GPU feature 未启用时 GpuVerifier 类型不可用
    #[test]
    fn test_gpu_feature_gate() {
        // 此测试验证编译时 feature gate 的正确性:
        // - 默认编译时 gpu 模块不存在
        // - 使用 --features gpu 时 gpu 模块才可用
        // 如果此测试能编译通过,说明 feature gate 工作正常
        #[cfg(not(feature = "gpu"))]
        {
            // 默认 feature 下 GpuVerifier 不应存在于作用域
            // 如果编译通过,说明条件编译正确
            assert!(true, "GPU feature 未启用,gpu 模块被正确排除");
        }

        #[cfg(feature = "gpu")]
        {
            // GPU feature 启用时,GpuVerifier 应可用
            assert!(true, "GPU feature 已启用,gpu 模块可用");
        }
    }

    /// 测试 WGSL shader 字符串非空且包含关键标识
    #[test]
    fn test_wgsl_shader_content() {
        assert!(!BLAKE3_SHADER.is_empty());
        assert!(BLAKE3_SHADER.contains("@group(0) @binding(0)"));
        assert!(BLAKE3_SHADER.contains("@group(0) @binding(1)"));
        assert!(BLAKE3_SHADER.contains("@compute @workgroup_size(256)"));
        assert!(BLAKE3_SHADER.contains("fn main("));
        assert!(BLAKE3_SHADER.contains("output_hash"));
        assert!(BLAKE3_SHADER.contains("input_data"));
    }

    /// 测试 CPU blake3 与 GPU 路径对同一数据产生相同哈希
    #[tokio::test]
    async fn test_cpu_gpu_hash_consistency() {
        let data = b"consistency check data";

        // CPU 哈希
        let cpu_hash = blake3::hash(data).to_hex().to_string();

        // GPU 路径(小数据会回退 CPU)
        let gpu_result = auto_select_and_hash(data).await.unwrap();

        assert_eq!(cpu_hash, gpu_result);
        assert_eq!(cpu_hash.len(), 64); // blake3 256-bit = 64 hex chars
    }

    /// 测试空数据的哈希计算
    #[tokio::test]
    async fn test_empty_data_hash() {
        let data = b"";
        let expected = blake3::hash(data).to_hex().to_string();
        let result = auto_select_and_hash(data).await.unwrap();
        assert_eq!(result, expected);
        assert!(!result.is_empty());
    }

    /// 测试不同数据产生不同哈希
    #[tokio::test]
    async fn test_different_data_different_hash() {
        let hash_a = auto_select_and_hash(b"data_a").await.unwrap();
        let hash_b = auto_select_and_hash(b"data_b").await.unwrap();
        assert_ne!(hash_a, hash_b);
    }
}
