import { createSignal, onMount, createEffect } from 'solid-js'
import type { ViewName } from './types'
import { api } from './api/invoke'
import { $tasks } from './stores/downloads'
import { useProgressListener } from './hooks/useTauriEvent'
import Layout from './components/Layout'
import TaskList from './components/TaskList'
import DetailPanel from './components/DetailPanel'
import SnifferPanel from './components/SnifferPanel'
import SettingsPanel from './components/SettingsPanel'

async function refreshTaskList() {
  try {
    const tasks = await api.getTaskList()
    $tasks.set(tasks)
  } catch (e) {
    console.error('刷新任务列表失败:', e)
  }
}

export default function App() {
  const [currentView, setCurrentView] = createSignal<ViewName>('downloads')

  useProgressListener()

  onMount(() => {
    refreshTaskList()
    api.subscribeProgress().catch(() => {})
  })

  createEffect(() => {
    const tasks = $tasks.get()
    const terminalStates = ['completed', 'failed', 'cancelled']
    const hasTerminal = tasks.some(t => terminalStates.includes(t.status))
    if (hasTerminal) {
      refreshTaskList()
    }
  })

  return (
    <Layout
      currentView={currentView()}
      onViewChange={setCurrentView}
      detail={currentView() === 'downloads' ? <DetailPanel /> : undefined}
    >
      {currentView() === 'downloads' && <TaskList />}
      {currentView() === 'sniffer' && <SnifferPanel />}
      {currentView() === 'settings' && <SettingsPanel />}
    </Layout>
  )
}
