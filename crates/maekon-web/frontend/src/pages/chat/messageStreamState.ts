import type { ChatMessage, SessionInfo } from './types'
import { now } from './utils'

export function finalizeThinkingMessage(items: ChatMessage[]): ChatMessage[] {
  const lastItem = items[items.length - 1]
  if (lastItem?.thinking && !lastItem.thinking.done) {
    return [...items.slice(0, -1), { ...lastItem, thinking: { ...lastItem.thinking, done: true } }]
  }
  return items
}

interface ApplyStreamMessageOptions {
  content: string
  done: boolean
  kind: 'delta' | 'result'
  extra?: Partial<ChatMessage>
  timestamp?: string
}

export function applyStreamMessage(items: ChatMessage[], options: ApplyStreamMessageOptions): ChatMessage[] {
  const base = finalizeThinkingMessage(items)
  const lastItem = base[base.length - 1]
  const canReconcileResult =
    options.kind === 'result' &&
    lastItem?.role === 'assistant' &&
    (lastItem.streaming ||
      !options.content ||
      options.content === lastItem.content ||
      options.content.startsWith(lastItem.content))

  if (lastItem?.role === 'assistant' && (lastItem.streaming || canReconcileResult)) {
    const content =
      options.kind === 'result'
        ? options.content && options.content.length >= lastItem.content.length
          ? options.content
          : lastItem.content
        : lastItem.content + options.content

    return [...base.slice(0, -1), { ...lastItem, content, streaming: !options.done, ...options.extra }]
  }

  return [
    ...base,
    {
      role: 'assistant',
      content: options.content,
      timestamp: options.timestamp ?? now(),
      streaming: !options.done,
      ...options.extra,
    },
  ]
}

export function completeSessionTurn(sessions: SessionInfo[], sessionId: string, completedAt: string): SessionInfo[] {
  return sessions.map((session) =>
    session.session_id === sessionId
      ? { ...session, turn_count: session.turn_count + 1, last_active: completedAt }
      : session,
  )
}
