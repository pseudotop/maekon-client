import { useCallback, useEffect, useRef, useState } from 'react'
import type { ChatMessage, OutboundMessage } from '../types'
import { now } from '../utils'

export function useMessageStream(activeId: string | null) {
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [sending, setSending] = useState(false)
  const scrollRef = useRef<HTMLDivElement>(null)
  const isNearBottom = useRef(true)
  const rafRef = useRef<number | null>(null)

  // Smart auto-scroll: RAF-throttled to avoid forced layout on every scroll event
  const handleScroll = useCallback(() => {
    if (rafRef.current) return
    rafRef.current = requestAnimationFrame(() => {
      const el = scrollRef.current
      if (el) {
        isNearBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 100
      }
      rafRef.current = null
    })
  }, [])

  // Clean up pending RAF on unmount
  useEffect(() => {
    return () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current)
    }
  }, [])

  // Auto-scroll when messages change
  useEffect(() => {
    if (isNearBottom.current) {
      scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: 'smooth' })
    }
  }, [])

  // SSE message listener
  useEffect(() => {
    if (!activeId) return
    let unlisten: (() => void) | null = null
    ;(async () => {
      const { listen } = await import('@tauri-apps/api/event')
      unlisten = await listen<OutboundMessage>(`ai-session:${activeId}`, ({ payload: p }) => {
        setMessages((prev) => {
          const finalizeThinking = (items: ChatMessage[]) => {
            const lastItem = items[items.length - 1]
            if (lastItem?.thinking && !lastItem.thinking.done) {
              return [...items.slice(0, -1), { ...lastItem, thinking: { ...lastItem.thinking, done: true } }]
            }
            return items
          }
          const appendStream = (items: ChatMessage[], c: string, done: boolean, extra?: Partial<ChatMessage>) => {
            const base = finalizeThinking(items)
            const lastItem = base[base.length - 1]
            if (lastItem?.role === 'assistant' && lastItem.streaming)
              return [...base.slice(0, -1), { ...lastItem, content: lastItem.content + c, streaming: !done, ...extra }]
            return [...base, { role: 'assistant' as const, content: c, timestamp: now(), streaming: !done, ...extra }]
          }
          const appendThinking = (items: ChatMessage[], c: string, done: boolean) => {
            const lastItem = items[items.length - 1]
            if (lastItem?.thinking && !lastItem.thinking.done) {
              return [
                ...items.slice(0, -1),
                {
                  ...lastItem,
                  content: lastItem.content + c,
                  thinking: { content: lastItem.thinking.content + c, done },
                },
              ]
            }
            return [
              ...items,
              {
                role: 'system' as const,
                content: c,
                timestamp: now(),
                thinking: { content: c, done },
              },
            ]
          }
          const appendToolCallDelta = (
            items: ChatMessage[],
            payload: Extract<OutboundMessage, { type: 'tool_call_delta' }>,
          ) => {
            const base = finalizeThinking(items)
            let existingIndex = -1
            for (let index = base.length - 1; index >= 0; index -= 1) {
              const current = base[index]?.tool_call_delta
              if (!current) continue
              if (payload.id && current.id === payload.id) {
                existingIndex = index
                break
              }
              if (!payload.id && current.index === payload.index) {
                existingIndex = index
                break
              }
            }

            if (existingIndex >= 0) {
              const existing = base[existingIndex]
              if (!existing?.tool_call_delta) {
                return base
              }

              return [
                ...base.slice(0, existingIndex),
                {
                  ...existing,
                  content: `${existing.content}${payload.arguments_chunk}`,
                  tool_call_delta: {
                    ...existing.tool_call_delta,
                    id: payload.id || existing.tool_call_delta.id,
                    name: payload.name || existing.tool_call_delta.name,
                    arguments: `${existing.tool_call_delta.arguments}${payload.arguments_chunk}`,
                  },
                },
                ...base.slice(existingIndex + 1),
              ]
            }

            return [
              ...base,
              {
                role: 'system' as const,
                content: payload.arguments_chunk,
                timestamp: now(),
                tool_call_delta: {
                  index: payload.index,
                  id: payload.id,
                  name: payload.name,
                  arguments: payload.arguments_chunk,
                },
              },
            ]
          }
          if (p.type === 'thinking') return appendThinking(prev, p.content, p.done)
          if (p.type === 'text') return appendStream(prev, p.content, p.done)
          if (p.type === 'result') {
            if (p.done) setSending(false)
            return appendStream(prev, p.content, p.done, {
              usage: p.usage,
              streaming: !p.done,
            })
          }
          if (p.type === 'tool_call_delta') return appendToolCallDelta(prev, p)
          if (p.type === 'tool_use')
            return [
              ...finalizeThinking(prev),
              {
                role: 'system',
                content: `Tool: ${p.tool} [${p.status}]`,
                timestamp: now(),
                tool_use: {
                  tool: p.tool,
                  status: p.status,
                  input: p.input as Record<string, unknown> | undefined,
                  result: p.result,
                },
              },
            ]
          if (p.type === 'error') {
            setSending(false)
            return [
              ...finalizeThinking(prev),
              {
                role: 'system',
                content: p.message,
                timestamp: now(),
                error: { code: p.code, message: p.message, retryable: p.retryable },
              },
            ]
          }
          if (p.type === 'control' && p.action === 'done') setSending(false)
          return prev
        })
      })
    })()
    return () => {
      unlisten?.()
    }
  }, [activeId])

  return { messages, setMessages, sending, setSending, scrollRef, handleScroll }
}
