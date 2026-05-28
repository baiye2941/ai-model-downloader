export type DownloadStatus = 'pending' | 'downloading' | 'paused' | 'completed' | 'failed' | 'cancelled'

export interface TaskInfo {
  id: string
  url: string
  file_name: string
  file_size: number
  downloaded: number
  speed: number
  status: DownloadStatus
  progress: number
  fragments_total: number
  fragments_done: number
  created_at: string
}

export interface AppConfig {
  download_dir: string
  max_concurrent_tasks: number
  max_concurrent_fragments: number
  max_connections_per_host: number
  enable_quic: boolean
  verify_checksum: boolean
}

export type SnifferResourceType = 'video' | 'audio' | 'document' | 'archive' | 'executable' | 'other'

export interface SnifferResource {
  url: string
  name: string
  type: SnifferResourceType
  size: number
  content_type?: string
  source_page?: string
}

export interface ProgressPayload {
  downloaded: number
  speed: number
  status: string
  fragmentsDone: number
}

export type ProgressEvent = Record<string, ProgressPayload>

export type ViewName = 'downloads' | 'sniffer' | 'settings'
