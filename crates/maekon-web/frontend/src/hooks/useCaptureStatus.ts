import { useEffect, useState } from 'react'

export interface CaptureStatus {
  paused: boolean
  indicator_visible: boolean
  consent_granted: boolean
  permitted: boolean
}

/**
 * Read the fail-closed desktop capture state and keep it synchronized with
 * tray/overlay changes. Outside Tauri the state stays unavailable instead of
 * assuming that capture is active.
 */
export function useCaptureStatus(): CaptureStatus | null {
  const [status, setStatus] = useState<CaptureStatus | null>(null)

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined

    ;(async () => {
      try {
        const [{ invoke }, { listen }] = await Promise.all([
          import('@tauri-apps/api/core'),
          import('@tauri-apps/api/event'),
        ])

        const current = await invoke<CaptureStatus>('get_capture_status')
        if (disposed) return
        setStatus(current)

        unlisten = await listen<CaptureStatus>('overlay:capture-state-changed', (event) => {
          if (!disposed) setStatus(event.payload)
        })
        if (disposed) unlisten()
      } catch {
        // Browser-only mode and unavailable IPC remain fail-closed/unknown.
        if (!disposed) setStatus(null)
      }
    })()

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  return status
}
