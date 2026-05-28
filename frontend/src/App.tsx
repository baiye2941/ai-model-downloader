import { createSignal } from 'solid-js'
import type { ViewName } from './types'
import Layout from './components/Layout'
import TaskList from './components/TaskList'

export default function App() {
  const [currentView, setCurrentView] = createSignal<ViewName>('downloads')

  return (
    <Layout currentView={currentView()} onViewChange={setCurrentView}>
      {currentView() === 'downloads' && <TaskList />}
      {currentView() === 'sniffer' && <div>SnifferPanel (待迁移)</div>}
      {currentView() === 'settings' && <div>SettingsPanel (待迁移)</div>}
    </Layout>
  )
}
