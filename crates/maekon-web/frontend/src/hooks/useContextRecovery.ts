import { useCallback, useRef, useState } from 'react'

/**
 * Current-context recovery generation hook (MK-CONTEXT-01, #8892).
 *
 * Invokes the consent-gated `request_current_context_suggestions` use case — the
 * one authoritative entry into a current-scene generation turn. It never
 * substitutes read-only scene analysis and never merely opens the existing
 * suggestion queue. The returned result is metadata only (status, provenance,
 * counts): the reviewable next-step text is loaded separately as a durable task
 * candidate, so raw OCR/window text never rides on this response.
 *
 * On a successful `generated` turn the backend has ingested a PROPOSED
 * `LOCAL_CURRENT_SCENE` task candidate; viewing it creates no to-do — only an
 * explicit confirm does. A single in-flight guard plus the backend's own
 * throttle keep a double-trigger from starting two turns.
 */

export type RecoveryStatus = 'generated' | 'no_candidate' | 'throttled' | 'analysis_unavailable' | 'consent_required'

export interface RecoveryProvenance {
  source_id: string
  observed_at: string
  captured_at: string
}

export interface RecoveryResult {
  status: RecoveryStatus
  reason: string | null
  admitted_count: number
  queue_count: number
  admitted_suggestion_ids: string[]
  missing_permissions: string[]
  provenance: RecoveryProvenance | null
}

export type RecoveryPhase = 'idle' | 'loading' | 'done' | 'error'

export interface UseContextRecovery {
  phase: RecoveryPhase
  result: RecoveryResult | null
  error: string | null
  generate: (sessionId?: string) => Promise<RecoveryResult | null>
  reset: () => void
}

/** Lazy-loaded invoke — graceful degradation outside Tauri (ADR-004). */
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: inv } = await import('@tauri-apps/api/core')
  return inv<T>(cmd, args)
}

export function useContextRecovery(): UseContextRecovery {
  const [phase, setPhase] = useState<RecoveryPhase>('idle')
  const [result, setResult] = useState<RecoveryResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const inFlight = useRef(false)

  const generate = useCallback(async (sessionId?: string) => {
    // UI-level double-trigger guard. The backend also fails a concurrent request
    // closed with `throttled`/`request_in_flight`, so this only avoids a wasted
    // round-trip on a rapid double-click.
    if (inFlight.current) return null
    inFlight.current = true
    setPhase('loading')
    setError(null)
    try {
      const res = await invoke<RecoveryResult>('request_current_context_suggestions', {
        sessionId: sessionId ?? null,
      })
      setResult(res)
      setPhase('done')
      return res
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setPhase('error')
      return null
    } finally {
      inFlight.current = false
    }
  }, [])

  const reset = useCallback(() => {
    setPhase('idle')
    setResult(null)
    setError(null)
  }, [])

  return { phase, result, error, generate, reset }
}
