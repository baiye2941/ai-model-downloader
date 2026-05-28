export type DownloadStatus = 'pending' | 'downloading' | 'paused' | 'completed' | 'failed' | 'cancelled'

export interface TaskInfo {
  id: string
  url: string
  fileName: string
  fileSize: number | null
  downloaded: number
  speed: number
  status: DownloadStatus
  progress: number
  fragmentsTotal: number
  fragmentsDone: number
  createdAt: string
}

export interface AppConfig {
  downloadDir: string
  maxConcurrentTasks: number
  maxConcurrentFragments: number
  maxConnectionsPerHost: number
  enableQuic: boolean
  verifyChecksum: boolean
}

export type SnifferResourceType = 'video' | 'audio' | 'document' | 'archive' | 'executable' | 'image' | 'other'

export interface SnifferResource {
  id: string
  url: string
  name: string
  type: SnifferResourceType
  size: number | null
  contentType?: string
  discoveredAt: number
  sourcePage?: string
}

export interface ProgressPayload {
  id: string
  progress: number
  downloaded: number
  speed: number
  status: string
  fragmentsDone: number
}

export type ProgressEvent = Record<string, ProgressPayload>

export type ViewName = 'downloads' | 'sniffer' | 'settings'
