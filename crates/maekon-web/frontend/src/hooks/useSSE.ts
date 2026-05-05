import { useCallback, useEffect, useRef, useState } from 'react'
import { isStandaloneModeEnabled } from '../api/standalone'
import { resolveApiUrl } from '../utils/api-base'

export interface MetricsUpdate {
  timestamp: string
  cpu_usage: number
  memory_percent: number
  memory_used: number
  memory_total: number
}

export interface FrameUpdate {
  id: number
  timestamp: string
  app_name: string
  window_title: string
  importance: number
  trigger_type: string
}

export interface IdleUpdate {
  is_idle: boolean
  idle_secs: number
}

export type RealtimeEvent =
  | { type: 'metrics'; data: MetricsUpdate }
  | { type: 'frame'; data: FrameUpdate }
  | { type: 'idle'; data: IdleUpdate }
  | { type: 'ping' }

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'error'

interface UseSSEOptions {
  autoReconnect?: boolean
  reconnectDelay?: number
  maxRetries?: number
}

interface UseSSEResult {
  status: ConnectionStatus
  latestMetrics: MetricsUpdate | null
  latestFrame: FrameUpdate | null
  idleState: IdleUpdate | null
  metricsHistory: MetricsUpdate[]
  connect: () => void
  disconnect: () => void
}

const MAX_HISTORY_SIZE = 60 // 60items data ( 5min)
/**
 *
 */
export function useSSE(options: UseSSEOptions = {}): UseSSEResult {
  const { autoReconnect = true, reconnectDelay = 3000, maxRetries = 10 } = options

  const [status, setStatus] = useState<ConnectionStatus>('disconnected')
  const [latestMetrics, setLatestMetrics] = useState<MetricsUpdate | null>(null)
  const [latestFrame, setLatestFrame] = useState<FrameUpdate | null>(null)
  const [idleState, setIdleState] = useState<IdleUpdate | null>(null)
  const [metricsHistory, setMetricsHistory] = useState<MetricsUpdate[]>([])

  const eventSourceRef = useRef<EventSource | null>(null)
  const retryCountRef = useRef(0)
  const reconnectTimeoutRef = useRef<number | null>(null)
  const connectTokenRef = useRef(0)

  const handleEvent = useCallback((event: MessageEvent) => {
    try {
      const data = JSON.parse(event.data) as RealtimeEvent

      switch (data.type) {
        case 'metrics':
          setLatestMetrics(data.data)
          setMetricsHistory((prev) => {
            if (prev.length >= MAX_HISTORY_SIZE) {
              return [...prev.slice(1), data.data]
            }
            return [...prev, data.data]
          })
          break
        case 'frame':
          setLatestFrame(data.data)
          break
        case 'idle':
          setIdleState(data.data)
          break
        case 'ping':
          break
      }
    } catch {
      console.error('SSE event parse error:', event.data)
    }
  }, [])

  const handleEventRef = useRef(handleEvent)
  handleEventRef.current = handleEvent

  const connectInternal = useCallback(async () => {
    if (eventSourceRef.current?.readyState === EventSource.OPEN) {
      return
    }

    const connectToken = ++connectTokenRef.current

    if (eventSourceRef.current) {
      eventSourceRef.current.close()
    }

    setStatus('connecting')
    const streamUrl = await resolveApiUrl('/api/stream')
    if (connectToken !== connectTokenRef.current) {
      return
    }

    const eventSource = new EventSource(streamUrl)
    if (connectToken !== connectTokenRef.current) {
      eventSource.close()
      return
    }
    eventSourceRef.current = eventSource

    eventSource.onopen = () => {
      if (connectToken !== connectTokenRef.current) {
        eventSource.close()
        return
      }
      setStatus('connected')
      retryCountRef.current = 0
    }

    const handler = (event: MessageEvent) => handleEventRef.current(event)
    eventSource.addEventListener('metrics', handler)
    eventSource.addEventListener('frame', handler)
    eventSource.addEventListener('idle', handler)
    eventSource.addEventListener('ping', handler)

    eventSource.onerror = () => {
      if (connectToken !== connectTokenRef.current) {
        eventSource.close()
        return
      }
      setStatus('error')
      eventSource.close()
      if (eventSourceRef.current === eventSource) {
        eventSourceRef.current = null
      }

      if (autoReconnect && retryCountRef.current < maxRetries) {
        retryCountRef.current++
        reconnectTimeoutRef.current = window.setTimeout(() => {
          void connectInternal()
        }, reconnectDelay)
      } else {
        setStatus('disconnected')
      }
    }
  }, [autoReconnect, reconnectDelay, maxRetries])

  const connect = useCallback(() => {
    void connectInternal()
  }, [connectInternal])

  const disconnect = useCallback(() => {
    connectTokenRef.current += 1

    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current)
      reconnectTimeoutRef.current = null
    }

    if (eventSourceRef.current) {
      eventSourceRef.current.close()
      eventSourceRef.current = null
    }

    setStatus('disconnected')
    retryCountRef.current = 0
  }, [])

  useEffect(() => {
    if (isStandaloneModeEnabled()) {
      setStatus('disconnected')
      return () => {}
    }
    connect()
    return () => {
      disconnect()
    }
  }, [connect, disconnect])

  return {
    status,
    latestMetrics,
    latestFrame,
    idleState,
    metricsHistory,
    connect,
    disconnect,
  }
}
