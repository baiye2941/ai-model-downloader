# QuantumFetch 前端重构规格

> 日期: 2026-05-28
> 状态: 已批准
> 范围: 渐进式阶段一 — 框架迁移 + TypeScript + Tauri 事件

## 1. 决策摘要

| 决策 | 选择 | 理由 |
|------|------|------|
| 框架 | Solid.js | 最小 bundle(~15KB)、编译时响应式、Tauri 最佳搭档 |
| 语言 | TypeScript | 类型安全 IPC、IDE 补全、编译时错误检测 |
| 构建 | Vite + vite-plugin-solid | Solid 官方推荐，Bun 兼容 |
| 状态 | nanostores | Solid 生态首选，极轻量响应式原子 |
| 风格 | 暗色科技风升级 | 保持现有 DNA，升级质感而非推翻 |
| 范围 | 渐进式 | 先迁移基础架构，保持功能不变 |

## 2. 当前前端问题

| # | 问题 | 严重性 |
|---|------|--------|
| 1 | 1769 行单文件 index.html，内联 CSS + JS | 严重 |
| 2 | 无 TypeScript，IPC 调用无类型检查 | 高 |
| 3 | 无组件框架，手动拼接 innerHTML | 高 |
| 4 | 嗅探面板硬编码 mock 数据 | 中 |
| 5 | 10 秒兜底轮询（虽有 Tauri 事件双模式） | 中 |
| 6 | 无响应式状态管理 | 中 |
| 7 | 无构建优化（tree-shaking、code-split） | 中 |
| 8 | src/ 目录为空 | 低 |

## 3. 目标架构

```
frontend/
  package.json
  vite.config.ts
  tsconfig.json
  index.html
  src/
    App.tsx
    main.tsx
    index.css
    types.ts
    api/
      invoke.ts
      events.ts
    stores/
      downloads.ts
      settings.ts
      sniffer.ts
    components/
      Layout.tsx
      Sidebar.tsx
      Topbar.tsx
      DownloadCard.tsx
      TaskList.tsx
      DetailPanel.tsx
      SnifferPanel.tsx
      SettingsPanel.tsx
      ProgressBar.tsx
      FragmentGrid.tsx
      Toggle.tsx
      SpeedGraph.tsx
    hooks/
      useTauriEvent.ts
    utils/
      format.ts
      icons.tsx
```

## 4. 依赖清单

```json
{
  "dependencies": {
    "solid-js": "^1.9",
    "@tauri-apps/api": "^2",
    "nanostores": "^0.10"
  },
  "devDependencies": {
    "vite": "^6",
    "vite-plugin-solid": "^2",
    "typescript": "^5.7",
    "@types/node": "^22"
  }
}
```

## 5. 设计令牌

保持现有 DNA，新增质感变量：

```css
:root {
  --bg: #0a0a0f;
  --surface: #111118;
  --surface-glass: rgba(17, 17, 24, 0.75);
  --border: #1a1a24;
  --border-light: #242430;
  --text: #d8d8e0;
  --text-2: #6a6a78;
  --text-3: #3a3a48;
  --accent: #10b981;
  --ok: #22c55e;
  --warn: #eab308;
  --err: #ef4444;
  --radius: 6px;
  --shadow: 0 1px 3px rgba(0,0,0,0.3);
  --font-sans: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
  --font-mono: 'JetBrains Mono', 'SF Mono', 'Cascadia Code', monospace;
}
```

## 6. 组件规格

### 6.1 Layout.tsx

三栏网格：sidebar(200px) + main(1fr) + detail(auto)。

```tsx
<div style="display: grid; grid-template-columns: 200px 1fr auto; height: 100vh;">
  <Sidebar />
  <Main />
  <DetailPanel />
</div>
```

### 6.2 DownloadCard.tsx

接收 `TaskInfo` prop，Solid 自动 DOM 更新替代手动 innerHTML。

| 字段 | 来源 | 说明 |
|------|------|------|
| name | task.file_name | 文件名 + 扩展标签 |
| status | task.status | 状态圆点 + 颜色 + 动画 |
| progress | task.downloaded / task.file_size | 进度条 |
| speed | task.speed | 速度值（绿色 mono） |
| fragments | task.fragments_done / task.fragments_total | 分片进度 |

### 6.3 TaskList.tsx

两个分组：活跃（downloading/paused/pending）+ 已完成（completed/failed/cancelled）。
使用 `<For>` 组件高效列表渲染，keyed by task.id。

### 6.4 DetailPanel.tsx

选中任务时显示右侧面板：文件大小、已下载、速度、分片数、协议、状态、FragmentGrid。

### 6.5 SnifferPanel.tsx

从 `sniffer` store 读取资源列表。Tauri 环境下通过 `get_sniffer_resources` IPC 获取真实数据，非 Tauri 环境显示空状态。

### 6.6 SettingsPanel.tsx

配置表单，从 `settings` store 读取，`update_config` IPC 写入。Toggle 组件支持键盘操作。

## 7. 状态管理

### 7.1 downloads.ts

```ts
// nanostores 原子
export const $tasks = atom<TaskInfo[]>([])
export const $selectedId = atom<string | null>(null)

// 派生 store
export const $activeTasks = computed($tasks, tasks =>
  tasks.filter(t => ['downloading','paused','pending'].includes(t.status))
)
export const $completedTasks = computed($tasks, tasks =>
  tasks.filter(t => ['completed','failed','cancelled'].includes(t.status))
)
export const $totalSpeed = computed($activeTasks, tasks =>
  tasks.reduce((sum, t) => sum + (t.speed || 0), 0)
)
```

### 7.2 settings.ts

```ts
export const $config = atom<AppConfig | null>(null)
export const $configLoading = atom(true)
```

### 7.3 sniffer.ts

```ts
export const $snifferResources = atom<SnifferResource[]>([])
export const $snifferActive = atom(true)
```

## 8. API 层

### 8.1 invoke.ts

类型安全的 Tauri IPC 封装：

```ts
export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return window.__TAURI__.core.invoke(cmd, args)
}

export const api = {
  createTask: (url: string) => invoke<string>('create_task', { url }),
  getTaskList: () => invoke<TaskInfo[]>('get_task_list'),
  getTaskDetail: (taskId: string) => invoke<TaskInfo>('get_task_detail', { taskId }),
  pauseTask: (taskId: string) => invoke<void>('pause_task', { taskId }),
  resumeTask: (taskId: string) => invoke<void>('resume_task', { taskId }),
  cancelTask: (taskId: string) => invoke<void>('cancel_task', { taskId }),
  deleteTask: (taskId: string) => invoke<void>('delete_task', { taskId }),
  getConfig: () => invoke<AppConfig>('get_config'),
  updateConfig: (config: Partial<AppConfig>) => invoke<void>('update_config', { config }),
  getSnifferResources: () => invoke<SnifferResource[]>('get_sniffer_resources'),
  subscribeProgress: () => invoke<void>('subscribe_progress'),
}
```

### 8.2 events.ts

Tauri 事件封装：

```ts
import { listen } from '@tauri-apps/api/event'

export function onProgressUpdate(handler: (payload: ProgressEvent) => void) {
  return listen<ProgressEvent>('progress-update', e => handler(e.payload))
}
```

## 9. TypeScript 接口

```ts
interface TaskInfo {
  id: string
  url: string
  file_name: string
  file_size: number
  downloaded: number
  speed: number
  status: 'pending' | 'downloading' | 'paused' | 'completed' | 'failed' | 'cancelled'
  progress: number
  fragments_total: number
  fragments_done: number
  created_at: string
}

interface AppConfig {
  download_dir: string
  max_concurrent_tasks: number
  max_concurrent_fragments: number
  max_connections_per_host: number
  enable_quic: boolean
  verify_checksum: boolean
}

interface SnifferResource {
  url: string
  name: string
  type: 'video' | 'audio' | 'document' | 'archive' | 'executable' | 'other'
  size: number
  content_type?: string
  source_page?: string
}

interface ProgressEvent {
  [taskId: string]: {
    downloaded: number
    speed: number
    status: string
    fragmentsDone: number
  }
}
```

## 10. 数据流

```
Tauri 后端
  ↓ invoke('get_task_list')
  ↓ listen('progress-update')
api/events 层
  ↓
nanostores ($tasks, $config, $snifferResources)
  ↓ computed 派生
  ↓
Solid 组件 (<For> 响应式渲染)
  ↓
DOM（Solid 编译时 DOM 操作，无 VDOM diff）
```

## 11. 阶段一实施清单

| # | 任务 | 依赖 |
|---|------|------|
| 1 | 安装依赖：solid-js, vite-plugin-solid, typescript, nanostores | 无 |
| 2 | 配置 vite.config.ts + tsconfig.json | 1 |
| 3 | 创建 main.tsx + App.tsx 骨架 | 2 |
| 4 | 迁移设计令牌到 index.css | 3 |
| 5 | 编写 types.ts (TaskInfo, AppConfig, SnifferResource) | 3 |
| 6 | 编写 api/invoke.ts + api/events.ts | 5 |
| 7 | 编写 stores/downloads.ts, settings.ts, sniffer.ts | 5 |
| 8 | 迁移 Layout.tsx + Sidebar.tsx + Topbar.tsx | 4,7 |
| 9 | 迁移 DownloadCard.tsx + ProgressBar.tsx + Toggle.tsx | 8 |
| 10 | 迁移 TaskList.tsx + DetailPanel.tsx + FragmentGrid.tsx | 9 |
| 11 | 迁移 SnifferPanel.tsx（删除 mock 数据） | 10 |
| 12 | 迁移 SettingsPanel.tsx | 10 |
| 13 | 编写 hooks/useTauriEvent.ts | 6 |
| 14 | 编写 utils/format.ts + utils/icons.tsx | 4 |
| 15 | Tauri 事件替换轮询（保留 30s 兜底） | 13 |
| 16 | 删除 index.html 内联 CSS/JS，仅保留入口引用 | 15 |
| 17 | 验证：cargo tauri dev 启动正常 | 16 |

## 12. 阶段二（后续，不在本次范围）

| 功能 | 优先级 |
|------|--------|
| 速度曲线图 (SpeedGraph.tsx, sparkline) | 高 |
| 拖拽 URL / 批量导入 | 高 |
| 剪贴板监控 | 高 |
| 自定义标题栏 (Tauri decorations: false) | 中 |
| 深色/浅色主题切换 | 中 |
| 噪点纹理背景 + 毛玻璃侧栏 | 中 |
| 系统托盘最小化 | 中 |
| 下载完成通知 | 中 |
| 国际化 (i18n) | 低 |

## 13. 约束

- MUST 使用 Bun（package.json scripts 用 bun）
- MUST 使用 Tauri v2
- MUST 使用 design-taste-frontend skill 确保 GUI 不模板化
- 渐进式：阶段一保持功能不变，仅升级底层架构
- 阶段一完成后，所有现有功能（下载列表、暂停/恢复/取消/删除、详情面板、嗅探面板、设置面板）必须正常工作
