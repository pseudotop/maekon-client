import { useEffect, useRef, useState } from 'react'
import type { UpdateStatus } from '../api/client'
import { isStandaloneModeEnabled } from '../api/standalone'
import { resolveApiUrl } from '../utils/api-base'

export type UpdateStreamStatus = 'connecting' | 'connected' | 'disconnected' | 'error'

export function useUpdateStream() {
  const [status, setStatus] = useState<UpdateStreamStatus>('disconnected')
  const [latest, setLatest] = useState<UpdateStatus | null>(null)
  const [lastEventAt, setLastEventAt] = useState<number | null>(null)
  const [lastError, setLastError] = useState<string | null>(null)
  const [retryCount, setRetryCount] = useState(0)
  const esRef = useRef<EventSource | null>(null)
  const retryRef = useRef<number | null>(null)
  const retries = useRef(0)

  useEffect(() => {
    if (isStandaloneModeEnabled()) {
      setStatus('disconnected')
      setLastError(null)
      setRetryCount(0)
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
      const streamUrl = await resolveApiUrl('/api/update/stream')
      if (disposed || currentToken !== connectToken) {
        return
      }

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
        retries.current = 0
        setRetryCount(0)
        setStatus('connected')
      }

      es.addEventListener('update_status', (event) => {
        try {
          const parsed = JSON.parse((event as MessageEvent).data) as UpdateStatus
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
    }
  }, [])

  return { status, latest, lastEventAt, lastError, retryCount }
}
