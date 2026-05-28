import { For } from 'solid-js'
import type { DownloadStatus } from '../types'

interface FragmentGridProps {
  total: number
  done: number
  status: DownloadStatus
}

export default function FragmentGrid(props: FragmentGridProps) {
  const blocks = (): Array<'done' | 'active' | 'pending'> => {
    const arr: Array<'done' | 'active' | 'pending'> = []
    for (let i = 0; i < props.total; i++) {
      if (i < props.done) arr.push('done')
      else if (i === props.done && props.status === 'downloading') arr.push('active')
      else arr.push('pending')
    }
    return arr
  }

  return (
    <div class="fragment-grid">
      <For each={blocks()}>
        {(cls) => <div class={`fragment-block ${cls}`} />}
      </For>
    </div>
  )
}
