import { atom, computed } from 'nanostores'
import type { TaskInfo, DownloadStatus } from '../types'

export const $tasks = atom<TaskInfo[]>([])
export const $selectedId = atom<string | null>(null)

const ACTIVE_STATUSES: DownloadStatus[] = ['downloading', 'paused', 'pending']
const COMPLETED_STATUSES: DownloadStatus[] = ['completed', 'failed', 'cancelled']

export const $activeTasks = computed($tasks, tasks =>
  tasks.filter(t => ACTIVE_STATUSES.includes(t.status))
)

export const $completedTasks = computed($tasks, tasks =>
  tasks.filter(t => COMPLETED_STATUSES.includes(t.status))
)

export const $totalSpeed = computed($activeTasks, tasks =>
  tasks.reduce((sum, t) => sum + (t.speed || 0), 0)
)
