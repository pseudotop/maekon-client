import { useEffect, useRef, useState } from 'react'
import type { UpdateStatus } from '../api/client'
import { isStandaloneModeEnabled } from '../api/standalone'
import { resolveApiUrl, resolveLocalAuthToken, setLocalAuthCookie, withLocalAuthQuery } from '../utils/api-base'

export type UpdateStreamStatus = 'connecting' | 'connected' | 'disconnected' | 'error'

export function useUpdateStream() {
  const [status, setStatus] = useState<UpdateStreamStatus>('disconnected')
  const [latest, setLatest] = useState<UpdateStatus | null>(null)
  const [lastEventAt, setLastEventAt] = useState<number | null>(null)
  const [recoveredAt, setRecoveredAt] = useState<number | null>(null)
  const [lastError, setLastError] = useState<string | null>(null)
  const [retryCount, setRetryCount] = useState(0)
  const esRef = useRef<EventSource | null>(null)
  const retryRef = useRef<number | null>(null)
  const retries = useRef(0)
  const latestRevision = useRef<number | null>(null)

  useEffect(() => {
    if (isStandaloneModeEnabled()) {
      setStatus('disconnected')
      setLastError(null)
      setRetryCount(0)
      setRecoveredAt(null)
      return () => {}
    }

    let disposed = false
    let connectToken = 0

    const connect = async () => {
      const currentToken = ++connectToken

      if (esRef.current) {
        esRef.current.close()
      }
      setStatus('connecting')
      setLastError(null)
      const baseStreamUrl = await resolveApiUrl('/api/update/stream')
      // E20-41 (#4833): EventSource sets no headers — auth via ?local_auth= query
      // (cross-origin Tauri) + cookie (same-origin browser). Query redacted in logs.
      await resolveLocalAuthToken()
      setLocalAuthCookie()
      const streamUrl = withLocalAuthQuery(baseStreamUrl)
      if (disposed || currentToken !== connectToken) {
        return
      }

      // Keep cross-origin Tauri query-auth non-credentialed while preserving
      // same-origin cookie auth through EventSource's default mode (#8202).
      const es = new EventSource(streamUrl)
      if (disposed || currentToken !== connectToken) {
        es.close()
        return
      }
      esRef.current = es

      es.onopen = () => {
        if (disposed || currentToken !== connectToken) {
          es.close()
          return
        }
        const recovered = retries.current > 0
        retries.current = 0
        setRetryCount(0)
        setStatus('connected')
        setRecoveredAt(recovered ? Date.now() : null)
      }

      es.addEventListener('update_status', (event) => {
        try {
          const parsed = JSON.parse((event as MessageEvent).data) as UpdateStatus
          if (latestRevision.current !== null && parsed.revision <= latestRevision.current) {
            return
          }
          latestRevision.current = parsed.revision
          setLatest(parsed)
          setLastEventAt(Date.now())
          setLastError(null)
        } catch {
          setLastError('stream_parse_error')
        }
      })

      es.onerror = () => {
        if (disposed || currentToken !== connectToken) {
          es.close()
          return
        }
        setStatus('error')
        setLastError('stream_connection_error')
        setRecoveredAt(null)
        es.close()
        if (esRef.current === es) {
          esRef.current = null
        }
        if (retries.current < 10) {
          retries.current += 1
          setRetryCount(retries.current)
          retryRef.current = window.setTimeout(() => {
            void connect()
          }, 2000)
        } else {
          setStatus('disconnected')
        }
      }
    }

    void connect()
    return () => {
      disposed = true
      connectToken += 1
      if (retryRef.current) {
        clearTimeout(retryRef.current)
      }
      if (esRef.current) {
        esRef.current.close()
      }
      setStatus('disconnected')
      setLastError(null)
      setRetryCount(0)
      setRecoveredAt(null)
    }
  }, [])

  return { status, latest, lastEventAt, recoveredAt, lastError, retryCount }
}
