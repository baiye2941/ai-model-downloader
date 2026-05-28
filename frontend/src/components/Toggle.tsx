import { createSignal } from 'solid-js'

interface ToggleProps {
  initial?: boolean
  ariaLabel: string
  onChange?: (value: boolean) => void
}

export default function Toggle(props: ToggleProps) {
  const [on, setOn] = createSignal(props.initial ?? false)

  function toggle() {
    const next = !on()
    setOn(next)
    props.onChange?.(next)
  }

  return (
    <div
      class={`toggle ${on() ? 'on' : ''}`}
      role="switch"
      aria-checked={on()}
      aria-label={props.ariaLabel}
      tabindex="0"
      onClick={toggle}
      onKeyDown={(e) => { if (e.key === ' ' || e.key === 'Enter') { e.preventDefault(); toggle() } }}
    />
  )
}
