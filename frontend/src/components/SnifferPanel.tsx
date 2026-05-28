import { For, Show, onMount } from 'solid-js'
import { useStore } from '@nanostores/solid'
import { $snifferResources, $snifferActive } from '../stores/sniffer'
import { $tasks } from '../stores/downloads'
import { api } from '../api/invoke'
import { formatSize } from '../utils/format'
import Toggle from './Toggle'
import type { SnifferResourceType } from '../types'

const TYPE_LABEL: Record<SnifferResourceType, string> = {
  video: 'VIDEO',
  audio: 'AUDIO',
  document: 'DOC',
  archive: 'ARC',
  executable: 'EXE',
  other: 'OTHER',
}

async function fetchResources() {
  try {
    const list = await api.getSnifferResources()
    $snifferResources.set(list)
  } catch {
    // Tauri 未就绪时静默忽略
  }
}

async function handleDownload(url: string) {
  try {
    await api.createTask(url)
    const list = await api.getTaskList()
    $tasks.set(list)
  } catch {
    // 静默忽略
  }
}

export default function SnifferPanel() {
  const resources = useStore($snifferResources)
  const active = useStore($snifferActive)

  onMount(() => {
    fetchResources()
  })

  return (
    <div>
      <div class="sniffer-header">
        <h2>资源嗅探</h2>
        <span class={`sniffer-status ${active() ? 'active' : 'inactive'}`}>
          {active() ? '监听中' : '已停止'}
        </span>
        <Toggle
          initial={active()}
          ariaLabel="嗅探开关"
          onChange={(val) => $snifferActive.set(val)}
        />
        <button class="btn btn-ghost" onClick={fetchResources} style={{ 'margin-left': 'auto' }}>
          刷新
        </button>
      </div>

      <Show
        when={resources().length > 0}
        fallback={
          <div class="empty-state">
            <svg class="empty-icon" viewBox="0 0 24 24">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
              <circle cx="12" cy="12" r="3" />
            </svg>
            <div class="empty-text">暂未嗅探到资源</div>
            <div class="empty-hint">浏览网页时自动捕获可下载资源</div>
          </div>
        }
      >
        <div class="sniffer-list">
          <For each={resources()}>
            {(res) => (
              <div class="sniffer-item">
                <span class={`sniffer-type type-${res.type}`}>
                  {TYPE_LABEL[res.type] || TYPE_LABEL.other}
                </span>
                <div class="sniffer-info">
                  <div class="sniffer-name">{res.name}</div>
                  <div class="sniffer-url">{res.url}</div>
                </div>
                <span class="sniffer-size">{formatSize(res.size)}</span>
                <div class="sniffer-actions">
                  <button
                    class="btn-sm btn-download-sm"
                    onClick={() => handleDownload(res.url)}
                  >
                    下载
                  </button>
                </div>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  )
}
