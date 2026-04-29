import { useCallback, useEffect, useRef } from 'react'

/**
 * Auto-dismiss timer. Calls `onDismiss` after `seconds` unless reset or cancelled.
 * Returns a `reset()` function to restart the timer (e.g., when LLM upgrade arrives).
 */
export function useAutoDismiss(active: boolean, seconds: number, onDismiss: () => void): { reset: () => void } {
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const onDismissRef = useRef(onDismiss)
  onDismissRef.current = onDismiss

  const clear = useCallback(() => {
    if (timerRef.current) {
      clearTimeout(timerRef.current)
      timerRef.current = null
    }
  }, [])

  const start = useCallback(() => {
    clear()
    timerRef.current = setTimeout(() => onDismissRef.current(), seconds * 1000)
  }, [clear, seconds])

  useEffect(() => {
    if (active) {
      start()
    } else {
      clear()
    }
    return clear
  }, [active, start, clear])

  return {
    reset: useCallback(() => {
      if (active) start()
    }, [active, start]),
  }
}
