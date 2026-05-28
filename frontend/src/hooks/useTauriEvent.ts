import { onCleanup } from 'solid-js'
import { onProgressUpdate } from '../api/events'
import { $tasks } from '../stores/downloads'

export function useProgressListener() {
  const unlistenPromise = onProgressUpdate((payload) => {
    if (!payload || typeof payload !== 'object') return

    const currentTasks = $tasks.get()
    let changed = false
    const updated = currentTasks.map(t => {
      const p = payload[t.id]
      if (!p) return t
      if (t.downloaded !== p.downloaded || t.speed !== p.speed || t.status !== p.status) {
        changed = true
        return {
          ...t,
          downloaded: p.downloaded ?? t.downloaded,
          speed: p.speed ?? t.speed,
          status: (p.status as t['status']) ?? t.status,
          fragmentsDone: p.fragmentsDone ?? t.fragmentsDone,
        }
      }
      return t
    })

    if (changed) {
      $tasks.set(updated)
    }
  })

  onCleanup(() => {
    unlistenPromise.then(fn => fn())
  })
}
