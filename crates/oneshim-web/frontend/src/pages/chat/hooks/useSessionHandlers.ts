import type React from 'react'
import { useCallback, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { downloadBlob } from '../../../api/client'
import { addToast } from '../../../hooks/useToast'
import { MAX_CACHED_SESSIONS } from '../constants'
import type { ChatMessage, MessageRecord, SessionConfig, SessionInfo, Transport } from '../types'
import { errorMessage, ipc, recordToChat } from '../utils'

interface UseSessionHandlersParams {
  activeId: string | null
  setActiveId: React.Dispatch<React.SetStateAction<string | null>>
  messages: ChatMessage[]
  setMessages: React.Dispatch<React.SetStateAction<ChatMessage[]>>
  sessions: SessionInfo[]
  setSessions: React.Dispatch<React.SetStateAction<SessionInfo[]>>
  setSessionLoadError: React.Dispatch<React.SetStateAction<string | null>>
  transport: Transport
  selectedHttpSurface: { surface_id: string } | null
  resolvedModel: string | undefined
  systemPrompt: string
  isHistorical: (s: SessionInfo) => boolean
}

export function useSessionHandlers({
  activeId,
  setActiveId,
  messages,
  setMessages,
  sessions,
  setSessions,
  setSessionLoadError,
  transport,
  selectedHttpSurface,
  resolvedModel,
  systemPrompt,
  isHistorical,
}: UseSessionHandlersParams) {
  const { t } = useTranslation()
  const messagesCache = useRef<Map<string, ChatMessage[]>>(new Map())

  const refresh = useCallback(() => {
    ipc<SessionInfo[]>('list_ai_sessions')
      .then((items) => {
        setSessions(items)
        setSessionLoadError(null)
      })
      .catch((e) => {
        const message = errorMessage(e, t('chat.refresh_failed', 'Failed to refresh AI sessions.'))
        console.warn('list_ai_sessions failed:', e)
        setSessionLoadError(message)
        addToast('error', message, 5000)
      })
  }, [t, setSessions, setSessionLoadError])

  const handleSelectSession = useCallback(
    async (id: string) => {
      if (activeId) messagesCache.current.set(activeId, messages)
      setActiveId(id)

      const cached = messagesCache.current.get(id)
      if (cached) {
        setMessages(cached)
      } else {
        try {
          const records = await ipc<MessageRecord[]>('load_session_messages', {
            sessionId: id,
          })
          const loaded = records.map(recordToChat)
          setMessages(loaded)
          messagesCache.current.set(id, loaded)
        } catch {
          setMessages([])
        }
      }

      if (messagesCache.current.size > MAX_CACHED_SESSIONS) {
        const oldest = messagesCache.current.keys().next().value
        if (oldest) messagesCache.current.delete(oldest)
      }
    },
    [activeId, messages, setActiveId, setMessages],
  )

  const handleCreate = useCallback(
    async (setCreating: (v: boolean) => void, setCreateError: (v: string | null) => void) => {
      setCreating(true)
      setCreateError(null)
      try {
        const info = await ipc<SessionInfo>('create_ai_session', {
          config: {
            transport,
            surface_id: transport === 'http_api' ? selectedHttpSurface?.surface_id : undefined,
            model: resolvedModel,
            system_prompt: systemPrompt || undefined,
            tools_enabled: true,
          } satisfies SessionConfig,
        })
        setSessions((p) => [info, ...p])
        setActiveId(info.session_id)
        setMessages([])
        setCreateError(null)
        setSessionLoadError(null)
      } catch (e) {
        console.warn('create_ai_session failed:', e)
        const message = errorMessage(e, t('chat.create_failed', 'Failed to create an AI session.'))
        setCreateError(message)
        addToast('error', message, 6000)
      }
      setCreating(false)
    },
    [
      transport,
      selectedHttpSurface,
      resolvedModel,
      systemPrompt,
      t,
      setSessions,
      setActiveId,
      setMessages,
      setSessionLoadError,
    ],
  )

  const handleDelete = useCallback(
    async (id: string) => {
      try {
        const session = sessions.find((s) => s.session_id === id)
        if (session && isHistorical(session)) {
          await ipc('delete_session_history', { sessionId: id })
        } else {
          await ipc('kill_ai_session', { sessionId: id })
        }
        setSessions((p) => p.filter((s) => s.session_id !== id))
        messagesCache.current.delete(id)
        if (activeId === id) {
          setActiveId(null)
          setMessages([])
        }
      } catch (e) {
        console.warn('kill_ai_session failed:', e)
        addToast('error', errorMessage(e, t('chat.delete_failed', 'Failed to delete the session.')), 5000)
      }
    },
    [activeId, sessions, t, isHistorical, setSessions, setActiveId, setMessages],
  )

  const handleExport = useCallback(
    (format: 'json' | 'markdown') => {
      if (!activeId || messages.length === 0) return
      const session = sessions.find((s) => s.session_id === activeId)
      const timestamp = new Date().toISOString().slice(0, 19).replace(/:/g, '-')

      if (format === 'json') {
        const payload = {
          session_id: activeId,
          provider: session?.provider_name,
          model: session?.model,
          transport: session?.transport,
          exported_at: new Date().toISOString(),
          messages: messages.map((m) => ({
            role: m.role,
            content: m.content,
            timestamp: m.timestamp,
            ...(m.usage && { usage: m.usage }),
            ...(m.thinking?.content && { thinking: m.thinking.content }),
            ...(m.tool_use && { tool_use: m.tool_use }),
          })),
        }
        const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' })
        downloadBlob(blob, `chat-${timestamp}.json`)
      } else {
        const lines: string[] = [
          `# Chat Export`,
          ``,
          `- **Session**: ${activeId}`,
          `- **Provider**: ${session?.provider_name ?? 'unknown'}`,
          `- **Model**: ${session?.model ?? 'default'}`,
          `- **Exported**: ${new Date().toISOString()}`,
          ``,
          `---`,
          ``,
        ]
        for (const m of messages) {
          const prefix = m.role === 'user' ? '## User' : m.role === 'assistant' ? '## Assistant' : '## System'
          lines.push(`${prefix} (${m.timestamp})`, ``)
          if (m.thinking?.content) {
            lines.push(`<details><summary>Thinking</summary>`, ``, m.thinking.content, ``, `</details>`, ``)
          }
          lines.push(m.content, ``)
          if (m.tool_use) {
            lines.push(`> Tool: **${m.tool_use.tool}** (${m.tool_use.status})`, ``)
          }
          if (m.usage) {
            lines.push(`*Tokens: ${m.usage.input_tokens} in / ${m.usage.output_tokens} out*`, ``)
          }
        }
        const blob = new Blob([lines.join('\n')], { type: 'text/markdown' })
        downloadBlob(blob, `chat-${timestamp}.md`)
      }
      addToast('success', t('chat.exported', 'Conversation exported'), 3000)
    },
    [activeId, messages, sessions, t],
  )

  const handleRetry = useCallback(async () => {
    if (!activeId) return
    try {
      const info = await ipc<SessionInfo>('retry_ai_session', { sessionId: activeId })
      setSessions((p) => p.map((s) => (s.session_id === info.session_id ? info : s)))
    } catch (e) {
      console.warn('retry_ai_session failed:', e)
      addToast('error', errorMessage(e, t('chat.retry_failed', 'Failed to retry the session.')), 5000)
    }
  }, [activeId, t, setSessions])

  return {
    refresh,
    handleSelectSession,
    handleCreate,
    handleDelete,
    handleExport,
    handleRetry,
  }
}
