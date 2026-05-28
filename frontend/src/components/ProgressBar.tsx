import type { DownloadStatus } from '../types'

interface ProgressBarProps {
  progress: number
  status: DownloadStatus
  label?: string
}

export default function ProgressBar(props: ProgressBarProps) {
  return (
    <div
      class="progress-bar"
      role="progressbar"
      aria-valuenow={props.progress}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-label={props.label}
    >
      <div class={`progress-fill ${props.status}`} style={{ width: `${props.progress}%` }} />
    </div>
  )
}
