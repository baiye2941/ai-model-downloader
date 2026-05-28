import { Show } from 'solid-js'
import { useStore } from '@nanostores/solid'
import { $tasks, $selectedId } from '../stores/downloads'
import { formatSize, formatSpeed, statusText } from '../utils/format'
import FragmentGrid from './FragmentGrid'

export default function DetailPanel() {
  const tasks = useStore($tasks)
  const selectedId = useStore($selectedId)

  const selectedTask = () => {
    const id = selectedId()
    if (!id) return null
    return tasks().find(t => t.id === id) ?? null
  }

  return (
    <Show when={selectedTask()} keyed>
      {(task) => (
        <div class="detail-panel" style={{ display: 'block' }}>
          <div class="panel-title">{task.fileName}</div>

          <div class="panel-row">
            <span class="panel-label">状态</span>
            <span class={`panel-value status-${task.status}`}>{statusText(task.status)}</span>
          </div>
          <div class="panel-row">
            <span class="panel-label">大小</span>
            <span class="panel-value mono">{formatSize(task.fileSize)}</span>
          </div>
          <div class="panel-row">
            <span class="panel-label">已下载</span>
            <span class="panel-value mono">{formatSize(task.downloaded)}</span>
          </div>
          <div class="panel-row">
            <span class="panel-label">进度</span>
            <span class="panel-value mono">{task.progress.toFixed(1)}%</span>
          </div>
          <Show when={task.speed > 0}>
            <div class="panel-row">
              <span class="panel-label">速度</span>
              <span class="panel-value mono speed-value">{formatSpeed(task.speed)}</span>
            </div>
          </Show>
          <div class="panel-row">
            <span class="panel-label">分片</span>
            <span class="panel-value mono">{task.fragmentsDone} / {task.fragmentsTotal}</span>
          </div>
          <div class="panel-row">
            <span class="panel-label">协议</span>
            <span class="panel-value mono">{new URL(task.url).protocol.replace(':', '').toUpperCase()}</span>
          </div>

          <Show when={task.fragmentsTotal > 0}>
            <FragmentGrid total={task.fragmentsTotal} done={task.fragmentsDone} status={task.status} />
          </Show>
        </div>
      )}
    </Show>
  )
}
