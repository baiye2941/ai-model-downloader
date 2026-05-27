//! BufferPool 分配与回收基准测试
//!
//! 测试不同 buffer 大小和池容量下的分配/回收性能。

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use qf_io::BufferPool;

/// 基准:从预填充池中分配 buffer
///
/// 测试池中有可用 buffer 时的 alloc 路径(pop_front),
/// 覆盖不同 buffer 大小(1KB / 4KB / 16KB / 64KB)。
fn bench_buffer_alloc(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_alloc");
    for size in [1024, 4096, 16384, 65536].iter() {
        group.bench_with_input(BenchmarkId::new("prefill_pool", size), size, |b, &size| {
            // 预填充池,确保 alloc 走快速路径
            let pool = BufferPool::with_prefill(size, 64);
            b.iter(|| pool.alloc());
        });
    }
    group.finish();
}

/// 基准:从空池中分配 buffer
///
/// 测试池为空时 alloc 走 fallback 路径(BytesMut::with_capacity),
/// 衡量堆分配开销。
fn bench_buffer_alloc_empty(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_alloc_empty");
    for size in [1024, 4096, 16384, 65536].iter() {
        group.bench_with_input(BenchmarkId::new("empty_pool", size), size, |b, &size| {
            let pool = BufferPool::new(size, 64);
            b.iter(|| pool.alloc());
        });
    }
    group.finish();
}

/// 基准:buffer 归还到池中
///
/// 测试 release 路径(清空 buffer 后 push_back),
/// 测量归还开销。
fn bench_buffer_release(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_release");
    for size in [1024, 4096, 16384, 65536].iter() {
        group.bench_with_input(BenchmarkId::new("release", size), size, |b, &size| {
            let pool = BufferPool::new(size, 128);
            b.iter(|| {
                let buf = pool.alloc();
                pool.release(buf);
            });
        });
    }
    group.finish();
}

/// 基准:alloc + release 完整循环
///
/// 模拟真实场景中 buffer 的分配-使用-归还全周期。
fn bench_buffer_alloc_release_cycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_cycle");
    for size in [4096, 16384, 65536].iter() {
        group.bench_with_input(BenchmarkId::new("cycle", size), size, |b, &size| {
            let pool = BufferPool::with_prefill(size, 64);
            b.iter(|| {
                let buf = pool.alloc();
                // 模拟轻量使用(不做实际 I/O,仅读取长度)
                let _len = buf.capacity();
                pool.release(buf);
            });
        });
    }
    group.finish();
}

/// 基准:不同池容量对 alloc 性能的影响
///
/// 固定 buffer 大小为 4KB,变化池容量(16 / 64 / 256 / 1024),
/// 观察 VecDeque 规模对锁竞争和缓存命中的影响。
fn bench_buffer_pool_capacity(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_pool_capacity");
    for cap in [16, 64, 256, 1024].iter() {
        group.bench_with_input(BenchmarkId::new("capacity", cap), cap, |b, &cap| {
            let pool = BufferPool::with_prefill(4096, cap);
            b.iter(|| {
                let buf = pool.alloc();
                pool.release(buf);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_buffer_alloc,
    bench_buffer_alloc_empty,
    bench_buffer_release,
    bench_buffer_alloc_release_cycle,
    bench_buffer_pool_capacity,
);
criterion_main!(benches);
