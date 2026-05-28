import { For } from 'solid-js'
import { useStore } from '@nanostores/solid'
import { $activeTasks, $totalSpeed } from '../stores/downloads'
import { formatSpeed } from '../utils/format'
import type { ViewName } from '../types'

interface SidebarProps {
  currentView: ViewName
  onViewChange: (v: ViewName) => void
}

const NAV_ITEMS: { view: ViewName; label: string; iconPath: string }[] = [
  { view: 'downloads', label: '下载列表', iconPath: 'M10 3v10m0 0l-3.5-3.5M10 13l3.5-3.5M4 17h12' },
  { view: 'sniffer', label: '资源嗅探', iconPath: 'M10 4C6 4 3 7.5 3 10.5S6 17 10 17s7-3.5 7-6.5S14 4 10 4z' },
  { view: 'settings', label: '设置', iconPath: 'M10 1.5v2m0 13v2M18.5 10h-2m-13 0h-2M15.66 4.34l-1.42 1.42M5.76 14.24l-1.42 1.42m11.32 0l-1.42-1.42M5.76 5.76L4.34 4.34' },
]

export default function Sidebar(props: SidebarProps) {
  const activeTasks = useStore($activeTasks)
  const totalSpeed = useStore($totalSpeed)

  return (
    <div class="sidebar" role="navigation" aria-label="主导航">
      <div class="sidebar-logo">QuantumFetch</div>
      <div class="sidebar-nav">
        <For each={NAV_ITEMS}>
          {(item) => (
            <div
              class={`nav-item ${props.currentView === item.view ? 'active' : ''}`}
              role="button"
              tabindex="0"
              aria-current={props.currentView === item.view ? 'page' : undefined}
              aria-label={item.label}
              onClick={() => props.onViewChange(item.view)}
              onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); props.onViewChange(item.view) } }}
            >
              <span class="nav-icon">
                <svg viewBox="0 0 20 20" aria-hidden="true">
                  <path d={item.iconPath} stroke="currentColor" fill="none" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
                </svg>
              </span>
              <span>{item.label}</span>
            </div>
          )}
        </For>
      </div>
      <div class="sidebar-stats" aria-live="polite" aria-label="全局统计">
        <div>活跃连接 <span class="mono">{activeTasks().filter(t => t.status === 'downloading').length}</span></div>
        <div>总速度 <span class="mono">{formatSpeed(totalSpeed())}</span></div>
        <div>队列任务 <span class="mono">{activeTasks().length}</span></div>
      </div>
    </div>
  )
}
