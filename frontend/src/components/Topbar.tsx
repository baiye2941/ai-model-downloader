import { createSignal } from 'solid-js'
import { api } from '../api/invoke'
import { $tasks } from '../stores/downloads'

export default function Topbar() {
  const [url, setUrl] = createSignal('')

  async function refreshTaskList() {
    try {
      const tasks = await api.getTaskList()
      $tasks.set(tasks)
    } catch (e) {
      console.error('刷新任务列表失败:', e)
    }
  }

  async function startDownload() {
    const u = url().trim()
    if (!u) return
    try {
      await api.createTask(u)
      setUrl('')
      refreshTaskList()
    } catch (e) {
      console.error('创建任务失败:', e)
    }
  }

  async function pauseAll() {
    const active = $tasks.get().filter(t => t.status === 'downloading' || t.status === 'pending')
    await Promise.allSettled(active.map(t => api.pauseTask(t.id).catch(() => {})))
    refreshTaskList()
  }

  return (
    <div class="topbar">
      <div class="url-bar">
        <input
          type="text"
          value={url()}
          onInput={(e) => setUrl(e.currentTarget.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') startDownload() }}
          placeholder="粘贴下载链接,支持 HTTP/HTTPS/FTP/QUIC..."
          aria-label="下载链接输入"
        />
        <button class="btn btn-primary" onClick={startDownload} aria-label="开始下载">开始下载</button>
      </div>
      <button class="btn btn-ghost" onClick={pauseAll} aria-label="暂停所有下载任务">全部暂停</button>
    </div>
  )
}
