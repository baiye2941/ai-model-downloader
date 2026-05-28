import type { JSX } from 'solid-js'
import { Show } from 'solid-js'
import Sidebar from './Sidebar'
import Topbar from './Topbar'
import type { ViewName } from '../types'

interface LayoutProps {
  currentView: ViewName
  onViewChange: (v: ViewName) => void
  children: JSX.Element
  detail?: JSX.Element
}

export default function Layout(props: LayoutProps) {
  return (
    <div style={{ display: 'grid', 'grid-template-columns': '200px 1fr auto', height: '100vh' }}>
      <Sidebar currentView={props.currentView} onViewChange={props.onViewChange} />
      <div class="main" style={{ display: 'flex', 'flex-direction': 'column', overflow: 'hidden' }}>
        <Show when={props.currentView === 'downloads'}>
          <Topbar />
        </Show>
        <div class="content" style={{ flex: 1, overflow: 'auto', padding: '16px 20px' }}>
          {props.children}
        </div>
      </div>
      {props.detail}
    </div>
  )
}
