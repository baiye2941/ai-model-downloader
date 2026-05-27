# CLAUDE.md

本文件为 Claude Code (claude.ai/code) 在本仓库中工作时提供指导。

## 项目概述

QuantumFetch -- 基于 Rust + Tauri 构建的新一代超高性能下载器,目标是全面超越 IDM。
核心能力:多线程分片下载、动态连接管理、协议级优化、智能调度与预测、超分片引擎、
多路径传输(MP-QUIC)、零拷贝存储引擎、P2SP 混合下载、GPU 加速校验、
内核网络栈旁路(Kernel Bypass)、io_uring、QUIC + 0-RTT、浏览器资源嗅探。

## 项目状态

项目处于初始化阶段,尚未创建 Cargo workspace。第一优先级是初始化 Rust 项目结构。

## 运行指令

```bash
# 构建
cargo build
cargo build --release

# 前端(Bun 而非 Node.js)
cd frontend && bun install && bun run dev

# Tauri 开发模式
cargo tauri dev

# 测试
cargo test --all
cargo test -p qf-core --lib
cargo test -p qf-core -- test_name --exact

# 代码检查
cargo clippy --all-targets --all-features -- -D warnings

# 格式化
cargo fmt --all -- --check

# 覆盖率
cargo llvm-cov --fail-under-lines 95

# 基准测试
cargo bench
```

## 架构总览

### 分层架构

```
+------------------------------------------+
|  GUI (Tauri + 前端)                       |
+------------------------------------------+
|  qf-app        应用层:任务管理、配置、UI 绑定  |
+------------------------------------------+
|  qf-scheduler  调度层:智能调度、带宽分配、预测   |
+------------------------------------------+
|  qf-engine     引擎层:分片引擎、连接池、P2SP   |
+------------------------------------------+
|  qf-protocol   协议层:HTTP/HTTPS/QUIC/FTP   |
+------------------------------------------+
|  qf-io         I/O 层:io_uring、零拷贝、落盘   |
+------------------------------------------+
|  qf-sniffer    嗅探层:浏览器资源拦截与解析      |
+------------------------------------------+
|  qf-crypto     校验层:GPU 加速哈希、完整性校验  |
+------------------------------------------+
```

### workspace 结构

```
QuantumFetch/
  Cargo.toml              # workspace 根
  crates/
    qf-core/              # 核心类型、trait 定义、错误体系
    qf-engine/            # 分片引擎、连接管理、MP-QUIC
    qf-scheduler/         # 智能调度器、带宽预测、优先级队列
    qf-io/                # io_uring 零拷贝存储引擎
    qf-protocol/          # HTTP/HTTPS/QUIC/FTP 协议实现
    qf-sniffer/           # 浏览器资源嗅探(集成 Playwright MCP)
    qf-crypto/            # GPU 加速校验( Vulkan compute / CUDA)
    qf-p2sp/              # P2SP 混合下载、DHT、Peer 发现
    qf-app/               # Tauri 应用入口、命令注册
  frontend/               # Tauri 前端(Bun)
  tests/                  # 集成测试
  benches/                # criterion 基准测试
```

### 关键设计决策

- **零拷贝管道**:网络收包->io_uring fixed buffer->文件写入,全程无用户态拷贝
- **超分片引擎**:动态粒度分片,根据带宽反馈实时调整分片大小与并发数
- **MP-QUIC 多路径**:单连接多路径传输,自动在 WiFi/5G/有线间切换与聚合
- **Kernel Bypass**:Linux 下可选 XDP/AF_XDP 旁路内核协议栈(需 root,按需启用)
- **GPU 校验**:通过 Vulkan Compute 或 CUDA 对分片做并行哈希校验
- **P2SP 混合**:CDN + DHT Peer 双源下载,自动测速选择最优源
- **智能调度**:历史数据带宽预测模型,提前分配连接资源
- **浏览器嗅探**:通过 Playwright MCP 拦截浏览器流量,自动捕获下载资源

## 工作规则

### 语言与格式
- 所有注释、文档、提交信息使用中文
- 代码标识符使用英文,遵循 Rust 命名规范
- 不使用 emoji

### 权限与执行
- 项目级操作直接执行,不询问用户确认
- 遇到问题自行思考解决,不中断等待用户输入
- 持续工作直到任务完成

### 并行开发
- 使用 Agent/Subagent 并行处理独立模块
- 写实现的 Agent 和写测试的 Agent 必须分离
- 使用 `subagent-driven-development` skill 执行多 Agent 工作流
- 使用 `dispatching-parallel-agents` skill 并行分派独立任务

### 代码质量
- 遵循 Rust 最佳实践:Result/Option、零成本抽象、trait 设计
- 使用 `rust-patterns` skill 确保惯用 Rust 写法
- cargo clippy 零警告
- 测试覆盖率目标 95% 以上(特殊情况除外)

### Git 提交
- 每完成一个有意义的阶段即提交
- 提交信息使用中文,遵循 `chinese-commit-conventions` skill
- 格式:`<类型>(<范围>): <简要描述>`

### 测试分离策略
- 实现代码和测试代码由不同 Agent 编写
- 使用 `tdd-guide` Agent 和 `test-driven-development` skill 强制执行 TDD
- 使用 `harness-builder` Agent 构建 TestHarness
- 六类测试:正常路径、空值、边界值、并发、外部故障、恶意输入
- Rust 工具:proptest 属性测试、tokio::test 异步测试、mockall 隔离外部依赖
- 覆盖率工具:cargo-llvm-cov

### 前端开发
- 使用 Bun,不使用 Node.js
- 使用 Tauri v2 框架
- 使用 `design-taste-frontend` skill 确保 GUI 与众不同,避免 AI 模板化设计
- GUI 要求美观现代化

## 项目级 Skills(位于 .claude/skills/)

### 开发流程类
- **brainstorming**:需求分析与设计规格,写代码前先想清楚
- **writing-plans**:把规格拆成可执行的实施步骤
- **executing-plans**:按计划逐步实施,每步验证
- **subagent-driven-development**:每个任务一个 Agent,两阶段审查(实现+规格审查+质量审查)
- **using-git-worktrees**:Git worktree 隔离开发
- **finishing-a-development-branch**:合并/PR/保留/丢弃收尾

### 代码质量类
- **test-driven-development**:强制 TDD,红-绿-重构循环
- **systematic-debugging**:四阶段调试法,根因追踪
- **requesting-code-review**:派遣审查 Agent
- **receiving-code-review**:处理审查反馈
- **verification-before-completion**:声称完成前必须运行验证命令
- **chinese-code-review**:中文代码审查规范

### Rust 专项
- **rust-patterns**:Rust 惯用模式(所有权、错误处理、trait、并发)
- **rust-testing**:Rust 测试模式(proptest、mockall、criterion)
- **security-review**:安全审查

### 中文规范
- **chinese-commit-conventions**:中文 Git 提交规范
- **chinese-documentation**:中文技术文档排版规范
- **chinese-git-workflow**:国内 Git 平台适配

### 前端
- **design-taste-frontend**:反 AI 审美前端设计,确保 GUI 不千篇一律

### 工具类
- **dispatching-parallel-agents**:并行分派独立任务
- **using-superpowers**:元技能,指导如何使用 skills
- **mcp-builder**:构建 MCP 服务器
- **workflow-runner**:多角色 YAML 工作流编排
- **writing-skills**:创建新 skill
- **coding-standards**:编码标准基线

## 项目级 Agents(位于 .claude/agents/)

| Agent | 用途 | 模型 |
|-------|------|------|
| rust-pro | Rust 专家(io_uring、QUIC、零拷贝、并发) | sonnet |
| rust-reviewer | Rust 代码审查(所有权、unsafe、并发、性能) | sonnet |
| performance-engineer | 性能工程师(io_uring、Kernel Bypass、GPU 加速) | opus |
| architect | 架构设计(trait 定义、接口设计、ADR) | opus |
| tdd-guide | TDD 专家(95% 覆盖率、六类测试) | sonnet |
| harness-builder | TestHarness 构建(mock、fixture、临时环境) | sonnet |

## MCP 服务器(.mcp.json)

| 服务器 | 用途 |
|--------|------|
| filesystem | 项目文件安全读写 |
| git | Git 仓库操作 |
| sequential-thinking | 复杂问题分步推理 |
| context7 | 实时拉取 Rust/Tauri/quinn 等库文档 |
| playwright | 浏览器自动化与资源嗅探 |

## 关键 Rust Crate

| 功能 | Crate |
|------|-------|
| 异步运行时 | tokio (multi-thread) |
| QUIC | quinn |
| io_uring | io-uring / tokio-uring |
| HTTP | reqwest (hyper) |
| Tauri | tauri v2 |
| GPU | vulkano / wgpu |
| 哈希 | sha2, blake3 |
| 序列化 | serde, serde_json |
| 属性测试 | proptest |
| 基准测试 | criterion |
| 错误处理 | thiserror, anyhow |
| 日志 | tracing, tracing-subscriber |
| DHT/P2P | librqbit(参考) / 自研 |
