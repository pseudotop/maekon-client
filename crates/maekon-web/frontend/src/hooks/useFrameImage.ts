/**
 * #8077-A: resilient frame-screenshot loading with truthful failure reasons.
 *
 * A raw `<img src>` cannot attach the per-session `X-Local-Auth` header, so in
 * the Tauri desktop app (where the WebView document is cross-origin to the
 * 127.0.0.1 API) screenshots must be fetched authenticated and displayed as a
 * blob object URL. In a same-origin browser preview the raw `<img>` still works
 * via the `maekon_local_auth` cookie, so we keep that fast path and only fall
 * back to an authenticated re-fetch when the raw load fails.
 *
 * Either way, when a frame is genuinely unavailable the re-fetch reads the
 * structured `code` the backend returns (`frames.image_missing` /
 * `frames.image_deleted` / `frames.image_load_failure`, or the re-auth gate's
 * `auth.reauth_required`) so the UI can show a real reason plus a recovery
 * action instead of the browser's broken-image glyph.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { authenticateCaptureHistory } from '../api/reauth'
import { resolveFrameImageRequestUrl, resolveImageUrl, withResolvedLocalAuthHeaders } from '../utils/api-base'
import { IS_TAURI } from '../utils/platform'

export type FrameImageReason = 'reauth' | 'deleted' | 'missing' | 'load_failure' | 'network' | 'unknown'

/** `raw`: same-origin `<img>` fast path. `loading`: authenticated fetch in flight. */
export type FrameImagePhase = 'raw' | 'loading' | 'ready' | 'error'

export interface FrameImageState {
  phase: FrameImagePhase
  /** URL to place in `<img src>` for the `raw`/`ready` phases; `null` otherwise. */
  src: string | null
  reason: FrameImageReason | null
  /** Whether a recovery re-fetch is worth offering (transient failures only). */
  retryable: boolean
  /** Wire to the rendered `<img onError>` — re-fetches (raw) or fails (ready). */
  notifyImgError: () => void
  /** Re-attempt the authenticated fetch (transient recovery). */
  retry: () => void
  /** Prompt OS biometric/PIN re-auth, then re-attempt on success. */
  reauthenticate: () => Promise<void>
  reauthPending: boolean
}

/** Read the structured failure `code` off an error response body (best effort). */
async function classifyImageResponse(res: Response): Promise<FrameImageReason> {
  let code = ''
  try {
    const body = (await res.clone().json()) as { code?: unknown }
    if (typeof body?.code === 'string') code = body.code
  } catch {
    // Non-JSON body — fall back to status-only classification below.
  }
  if (res.status === 403) return code === 'auth.reauth_required' ? 'reauth' : 'unknown'
  if (res.status === 404) return code === 'frames.image_deleted' ? 'deleted' : 'missing'
  if (res.status >= 500) return 'load_failure'
  return 'unknown'
}

const RETRYABLE_REASONS: ReadonlySet<FrameImageReason> = new Set<FrameImageReason>([
  'load_failure',
  'network',
  'unknown',
])

export function useFrameImage(imageUrl: string | null | undefined): FrameImageState {
  const rawUrl = resolveImageUrl(imageUrl)
  const [phase, setPhase] = useState<FrameImagePhase>(() => (IS_TAURI ? 'loading' : 'raw'))
  const [objectUrl, setObjectUrl] = useState<string | null>(null)
  const [reason, setReason] = useState<FrameImageReason | null>(null)
  const [reauthPending, setReauthPending] = useState(false)

  const mountedRef = useRef(true)
  const phaseRef = useRef(phase)
  phaseRef.current = phase
  const objectUrlRef = useRef<string | null>(null)
  // Monotonic token so a stale in-flight fetch never clobbers a newer attempt.
  const attemptRef = useRef(0)

  const revoke = useCallback(() => {
    if (objectUrlRef.current) {
      URL.revokeObjectURL(objectUrlRef.current)
      objectUrlRef.current = null
    }
  }, [])

  const runFetch = useCallback(async () => {
    if (!imageUrl) {
      setReason('missing')
      setPhase('error')
      return
    }
    const attempt = ++attemptRef.current
    setPhase('loading')
    setReason(null)
    try {
      // #11648: resolve the live local-API port at request time. The frame-list
      // request already follows this async path; using the synchronous raw
      // image URL here could pin thumbnails to another coexisting Maekon
      // identity on the default listener.
      const requestUrl = await resolveFrameImageRequestUrl(imageUrl)
      if (attempt !== attemptRef.current || !mountedRef.current) return
      if (!requestUrl) {
        setReason('missing')
        setPhase('error')
        return
      }
      const res = await fetch(requestUrl, await withResolvedLocalAuthHeaders({ method: 'GET' }))
      if (attempt !== attemptRef.current || !mountedRef.current) return
      if (res.ok) {
        const blob = await res.blob()
        if (attempt !== attemptRef.current || !mountedRef.current) return
        revoke()
        const obj = URL.createObjectURL(blob)
        objectUrlRef.current = obj
        setObjectUrl(obj)
        setPhase('ready')
      } else {
        setReason(await classifyImageResponse(res))
        setPhase('error')
      }
    } catch {
      if (attempt !== attemptRef.current || !mountedRef.current) return
      setReason('network')
      setPhase('error')
    }
  }, [imageUrl, revoke])

  // Reset when the target frame changes. Tauri fetches immediately (raw `<img>`
  // would 401 cross-origin); browser starts on the same-origin raw fast path.
  useEffect(() => {
    mountedRef.current = true
    attemptRef.current++
    revoke()
    setObjectUrl(null)
    setReason(null)
    if (IS_TAURI) {
      void runFetch()
    } else {
      setPhase('raw')
    }
    return () => {
      mountedRef.current = false
    }
    // `runFetch` already closes over `rawUrl`, so it re-runs when the frame changes.
  }, [runFetch, revoke])

  // Release the last object URL on unmount.
  useEffect(() => () => revoke(), [revoke])

  const notifyImgError = useCallback(() => {
    // A blob (`ready`) image that fails is a genuine decode failure; a raw
    // (`raw`) image that fails triggers the authenticated re-fetch/classify.
    if (phaseRef.current === 'ready') {
      setReason('load_failure')
      setPhase('error')
    } else {
      void runFetch()
    }
  }, [runFetch])

  const retry = useCallback(() => {
    void runFetch()
  }, [runFetch])

  const reauthenticate = useCallback(async () => {
    setReauthPending(true)
    try {
      const outcome = await authenticateCaptureHistory({ method: 'biometric' })
      if (outcome.outcome === 'authenticated') {
        await runFetch()
      }
    } catch {
      // Keep the error state; the user can retry the prompt.
    } finally {
      if (mountedRef.current) setReauthPending(false)
    }
  }, [runFetch])

  const src = phase === 'raw' ? rawUrl : phase === 'ready' ? objectUrl : null

  return {
    phase,
    src,
    reason,
    retryable: reason !== null && RETRYABLE_REASONS.has(reason),
    notifyImgError,
    retry,
    reauthenticate,
    reauthPending,
  }
}
