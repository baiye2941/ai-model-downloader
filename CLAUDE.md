# CLAUDE.md

## 技术栈

Rust + Tauri v2 超高性能下载器。Cargo workspace 10 crate,604 测试,零 clippy 警告。

依赖层序: qf-core > qf-protocol > qf-engine > qf-io > qf-app,其余(qf-scheduler/qf-crypto/qf-p2sp/qf-sniffer/qf-store)为独立层。

## 命令

```bash
cargo build --all                                              # 构建
cargo test --all                                               # 测试
cargo test -p qf-core -- test_name --exact                    # 单测过滤
cargo clippy --all-targets --all-features -- -D warnings      # 零警告
cargo fmt --all -- --check                                     # 格式检查
cargo tauri dev                                                # Tauri 开发
cd frontend && bun install && bun run dev                      # 前端
```

## 关键规则

- cargo clippy MUST 零警告(CI 用 `-D warnings`,警告即报错)
- 测试覆盖率 MUST >= 95%
- 所有 unsafe 代码 MUST 有 Safety 注释
- 注释/文档/提交信息使用中文,代码标识符使用英文,不使用 emoji
- 前端 MUST 使用 Bun + Tauri v2,MUST 使用 design-taste-frontend skill
- 会话开始时若 `target/` 超过 5GB,执行 `cargo clean`

## Git 工作流

- 每完成一个有意义的阶段即提交,格式:`<类型>(<范围>): <简要描述>`(中文)
- 提交前完整流程:`cargo fmt --all` -> `cargo build --all`(零警告) -> `cargo test --all`(全通过)
- 提交后 MUST 推送:`git push`
- 修复 CI/CD 报错后再推送

## 并行开发

- 使用 subagent-driven-development skill 执行多 Agent 工作流
- 使用 dispatching-parallel-agents skill 并行分派独立任务
- 写实现的 Agent 和写测试的 Agent MUST 分离

## 经验教训(NEVER 重复这些错误)

- NEVER 在 task_fn 中使用模拟下载。REAL DOWNLOAD MUST 通过 DownloadTask::run() 激活
- NEVER 自建 reqwest Client 绕过 qf-protocol。MUST 使用 qf-protocol 的 HttpClient
- NEVER 在多个 crate 中各自维护 ConnectionPool。连接池 MUST 为全局单例通过 qf-engine 导出
- NEVER 在分片下载 spawn 内绕过状态机(直接赋值 downloaded/state)。MUST 通过 complete_download / complete_download_fast 设置 last_duration
- NEVER 将调度器 recommendation 只用于 debug 打印。recommendation.fragment_size MUST 传入 plan_fragments
- NEVER 在 validate_save_path 中对不存在的文件返回未 canonicalize 的原始路径。MUST 返回 canonical_parent.join(file_name)
- sanitize_filename 的 .. 移除 MUST 使用 O(n) 单次扫描,禁止 while-replace O(n^2)
- paused 状态 MUST 有时间上限,不能永久暂停
- 所有 crate 从 qf-core::config 读取配置,禁止各自解析 env
- Storage trait 和 I/O 实现 MUST 对齐契约(支持并发写入)

## Skills 与 Agents

@.claude/skills/ @.claude/agents/

## MCP 服务器

@.mcp.json
