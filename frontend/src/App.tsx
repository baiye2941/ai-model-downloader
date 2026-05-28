import { createSignal } from 'solid-js'
import { Show } from 'solid-js'
import type { ViewName } from './types'
import Layout from './components/Layout'
import TaskList from './components/TaskList'
import DetailPanel from './components/DetailPanel'
import SnifferPanel from './components/SnifferPanel'
import SettingsPanel from './components/SettingsPanel'

export default function App() {
  const [currentView, setCurrentView] = createSignal<ViewName>('downloads')

  return (
    <Layout
      currentView={currentView()}
      onViewChange={setCurrentView}
      detail={
        <Show when={currentView() === 'downloads'}>
          <DetailPanel />
        </Show>
      }
    >
      {currentView() === 'downloads' && <TaskList />}
      {currentView() === 'sniffer' && <SnifferPanel />}
      {currentView() === 'settings' && <SettingsPanel />}
    </Layout>
  )
}
