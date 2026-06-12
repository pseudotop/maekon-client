import type { CaptureStatePayload } from '../types'

interface TrackingBorderProps {
  captureState: CaptureStatePayload
  windowLabel?: string
}

function borderClassForWindow(windowLabel?: string) {
  switch (windowLabel) {
    case 'tracking-border-top':
      return 'top-0 left-0 right-0 h-[3px] bg-brand-signal'
    case 'tracking-border-bottom':
      return 'bottom-0 left-0 right-0 h-[3px] bg-brand-signal'
    case 'tracking-border-left':
      return 'left-0 top-0 bottom-0 w-[3px] bg-brand-signal'
    case 'tracking-border-right':
      return 'right-0 top-0 bottom-0 w-[3px] bg-brand-signal'
    default:
      return 'inset-0 border-[3px] border-brand-signal'
  }
}

export function TrackingBorder({ captureState, windowLabel }: TrackingBorderProps) {
  if (captureState.paused || !captureState.indicator_visible) return null

  return (
    <div
      aria-hidden="true"
      data-testid="tracking-border"
      className={`pointer-events-none fixed z-detection animate-tracking-blink ${borderClassForWindow(windowLabel)}`}
      style={{
        boxShadow: 'inset 0 0 20px rgb(var(--brand-signal) / 0.45)',
      }}
    />
  )
}
