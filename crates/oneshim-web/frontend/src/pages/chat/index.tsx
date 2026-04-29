import { ChevronDown, Download, FileText, Loader2, MessageSquarePlus, RefreshCw, Search } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Alert, Button, EmptyState, Input } from '../../components/ui'
import { defaultSurfaceModel } from '../../features/providerSurfaces'
import { addToast } from '../../hooks/useToast'
import { colors, iconSize, interaction, motion, radius, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { ChatInput } from './ChatInput'
import { ChatSidebar } from './ChatSidebar'
import { STATE_DOT } from './constants'
import { useAudioCapture } from './hooks/useAudioCapture'
import { useMessageStream } from './hooks/useMessageStream'
import { useSessionHandlers } from './hooks/useSessionHandlers'
import { useSessionSetup } from './hooks/useSessionSetup'
import { MessageBubble } from './MessageBubble'
import type { AttachmentPayload, Transport } from './types'
import { errorMessage, ipc, now, parseDataUrl, parseOptionalJsonValue, parseOptionalToolDefinitions } from './utils'

export default function Chat() {
  const { t } = useTranslation()
  const isHistorical = useCallback((s: { state: string }) => s.state === 'terminated', [])

  // ---- Session setup (provider catalog, surfaces, sessions list) ----
  const {
    httpApiSurfaces,
    httpSurfaceId,
    setHttpSurfaceId,
    sessions,
    setSessions,
    tokenUsage,
    sessionLoadError,
    setSessionLoadError,
  } = useSessionSetup()

  // ---- Local UI state ----
  const [activeId, setActiveId] = useState<string | null>(() => {
    // Auto-select session from ?sid= query parameter (e.g. explain-in-chat navigation)
    const params = new URLSearchParams(window.location.search)
    return params.get('sid')
  })
  const [input, setInput] = useState('')
  const [transport, setTransport] = useState<Transport>('subprocess')
  const [creating, setCreating] = useState(false)
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [systemPrompt, setSystemPrompt] = useState('')
  const [modelOverride, setModelOverride] = useState('')
  const [showMessagePayload, setShowMessagePayload] = useState(false)
  const [contextRegime, setContextRegime] = useState('')
  const [contextActiveApp, setContextActiveApp] = useState('')
  const [toolsJson, setToolsJson] = useState('')
  const [responseFormatJson, setResponseFormatJson] = useState('')
  const [searchQuery, setSearchQuery] = useState('')
  const [searchOpen, setSearchOpen] = useState(false)
  const [attachments, setAttachments] = useState<Array<{ name: string; type: string; data: string }>>([])
  const [createError, setCreateError] = useState<string | null>(null)
  const [requestingSuggestions, setRequestingSuggestions] = useState(false)
  const [suggestionCooldown, setSuggestionCooldown] = useState(false)

  // ---- Message stream (SSE listener, scroll handling) ----
  const { messages, setMessages, sending, setSending, scrollRef, handleScroll } = useMessageStream(activeId)

  // ---- Derived values ----
  const selectedHttpSurface = useMemo(
    () => httpApiSurfaces.find((surface) => surface.surface_id === httpSurfaceId) ?? httpApiSurfaces[0] ?? null,
    [httpApiSurfaces, httpSurfaceId],
  )
  const resolvedModel = useMemo(() => {
    const override = modelOverride.trim()
    if (override) return override
    if (transport === 'http_api') {
      return defaultSurfaceModel(selectedHttpSurface ?? undefined, 'llm_api') ?? undefined
    }
    return undefined
  }, [modelOverride, selectedHttpSurface, transport])
  const messageContext = useMemo(() => {
    const regime = contextRegime.trim()
    const activeApp = contextActiveApp.trim()
    if (!regime && !activeApp) return undefined
    return {
      regime: regime || undefined,
      active_app: activeApp || undefined,
    }
  }, [contextActiveApp, contextRegime])
  const parsedTools = useMemo(() => parseOptionalToolDefinitions(toolsJson), [toolsJson])
  const parsedResponseFormat = useMemo(() => parseOptionalJsonValue(responseFormatJson), [responseFormatJson])
  const payloadInvalid = parsedTools.error || parsedResponseFormat.error
  const messagePayloadCount = useMemo(() => {
    let count = 0
    if (messageContext) count += 1
    if (toolsJson.trim()) count += 1
    if (responseFormatJson.trim()) count += 1
    return count
  }, [messageContext, responseFormatJson, toolsJson])

  useEffect(() => {
    if (payloadInvalid) setShowMessagePayload(true)
  }, [payloadInvalid])

  // Task 7: Listen for auto-extracted suggestions from AI responses
  useEffect(() => {
    if (!activeId) return
    let unlisten: (() => void) | null = null
    ;(async () => {
      const { listen } = await import('@tauri-apps/api/event')
      unlisten = await listen<{ count: number; sessionId: string }>('chat:suggestions-extracted', ({ payload }) => {
        if (payload.sessionId === activeId) {
          addToast('info', `${payload.count} suggestion${payload.count !== 1 ? 's' : ''} added from this conversation`)
        }
      })
    })()
    return () => {
      unlisten?.()
    }
  }, [activeId])

  // ---- Session handlers ----
  const {
    refresh,
    handleSelectSession,
    handleCreate: handleCreateInner,
    handleDelete,
    handleExport,
    handleRetry,
  } = useSessionHandlers({
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
  })

  const handleCreate = useCallback(() => {
    if (creating) return
    handleCreateInner(setCreating, setCreateError)
  }, [creating, handleCreateInner])

  const handleRename = useCallback(
    async (id: string, title: string) => {
      try {
        await ipc('rename_ai_session', { sessionId: id, newTitle: title })
        setSessions((prev) => prev.map((s) => (s.session_id === id ? { ...s, title } : s)))
      } catch (e) {
        console.warn('rename_ai_session failed:', e)
        addToast('error', errorMessage(e, 'Failed to rename session'), 5000)
      }
    },
    [setSessions],
  )

  const handleRequestSuggestions = useCallback(async () => {
    if (!activeId) return
    setRequestingSuggestions(true)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const count = await invoke<number>('request_chat_suggestions', { sessionId: activeId })
      addToast('success', `${count} suggestion${count !== 1 ? 's' : ''} generated`)
      setSuggestionCooldown(true)
      setTimeout(() => setSuggestionCooldown(false), 5000)
    } catch (e) {
      addToast('error', `Failed to get suggestions: ${errorMessage(e, 'Failed to get suggestions')}`)
    } finally {
      setRequestingSuggestions(false)
    }
  }, [activeId])

  // ---- Active session state ----
  const active = sessions.find((s) => s.session_id === activeId)
  const MAX_VISIBLE_MESSAGES = 500
  const isTruncated = messages.length > MAX_VISIBLE_MESSAGES
  const visibleMessages = isTruncated ? messages.slice(-MAX_VISIBLE_MESSAGES) : messages
  const createDisabled = creating || (transport === 'http_api' && !selectedHttpSurface)
  const activeSession = sessions.find((s) => s.session_id === activeId)
  const isReadOnly = activeSession ? isHistorical(activeSession) : false

  // ---- Audio capture ----
  const {
    audioAvailable,
    audioTooltip,
    micMode,
    vadState,
    recording,
    transcribing,
    handleMicDown,
    handleMicUp,
    handleVadToggle,
  } = useAudioCapture(isReadOnly, setInput)

  // ---- Search ----
  const searchMatchCount = useMemo(() => {
    if (!searchQuery) return 0
    const q = searchQuery.toLowerCase()
    return messages.filter((m) => m.content.toLowerCase().includes(q)).length
  }, [messages, searchQuery])

  // ---- Send ----
  const handleSend = useCallback(async () => {
    if ((!input.trim() && attachments.length === 0) || !activeId || sending || payloadInvalid) return
    const text = input.trim()
    const attachmentPayload: AttachmentPayload[] = attachments.map((attachment) => {
      const parsed = parseDataUrl(attachment.data)
      if (attachment.type.startsWith('image/')) {
        return {
          kind: 'image',
          mime: parsed?.mime || attachment.type || 'application/octet-stream',
          data: parsed?.data ?? null,
          path: null,
        }
      }

      return {
        kind: 'file',
        path: attachment.name,
        mime: parsed?.mime || attachment.type || null,
        data: parsed?.data ?? null,
      }
    })
    const attachmentSummary = attachments
      .map((attachment) =>
        attachment.type.startsWith('image/')
          ? `[Image attachment: ${attachment.name}]`
          : `[Attachment: ${attachment.name}]`,
      )
      .join('\n')
    const displayText =
      attachments.length > 0 ? [attachmentSummary, text].filter((section) => section.length > 0).join('\n') : text
    setInput('')
    setAttachments([])
    // Reset textarea height after clearing input
    const ta = document.querySelector<HTMLTextAreaElement>('form textarea')
    if (ta) ta.style.height = 'auto'
    setMessages((p) => [...p, { role: 'user', content: displayText, timestamp: now() }])
    setSending(true)
    try {
      await ipc('send_session_message', {
        sessionId: activeId,
        message: text,
        attachments: attachmentPayload,
        tools: parsedTools.value,
        context: messageContext,
        responseFormat: parsedResponseFormat.value,
      })
    } catch (e) {
      console.warn('send_session_message failed:', e)
      setSending(false)
      addToast('error', errorMessage(e, t('chat.send_failed', 'Failed to send the message.')), 5000)
    }
  }, [
    input,
    activeId,
    sending,
    attachments,
    payloadInvalid,
    parsedTools.value,
    messageContext,
    parsedResponseFormat.value,
    t,
    setMessages,
    setSending,
  ])

  const sendDisabled = (!input.trim() && attachments.length === 0) || sending || payloadInvalid || isReadOnly

  return (
    <div className="flex h-full min-h-0">
      <ChatSidebar
        sessions={sessions}
        activeId={activeId}
        transport={transport}
        setTransport={setTransport}
        httpApiSurfaces={httpApiSurfaces}
        selectedHttpSurface={selectedHttpSurface}
        setHttpSurfaceId={setHttpSurfaceId}
        modelOverride={modelOverride}
        setModelOverride={setModelOverride}
        systemPrompt={systemPrompt}
        setSystemPrompt={setSystemPrompt}
        showAdvanced={showAdvanced}
        setShowAdvanced={setShowAdvanced}
        creating={creating}
        createDisabled={createDisabled}
        onRefresh={refresh}
        onSelectSession={handleSelectSession}
        onCreate={handleCreate}
        onDelete={handleDelete}
        onRename={handleRename}
        isHistorical={isHistorical}
      />

      {/* Main area */}
      <div className="flex min-w-0 flex-1 flex-col bg-surface-sunken">
        {!activeId ? (
          <div className="flex flex-1 flex-col items-center justify-center">
            <EmptyState
              icon={<MessageSquarePlus className="h-8 w-8" />}
              title={t('emptyState.chat.title')}
              description={t('emptyState.chat.description')}
              action={{
                label: creating ? t('common.loading') : t('emptyState.chat.action'),
                onClick: handleCreate,
              }}
            />
            {createError && (
              <div className="mt-2 w-full max-w-md px-6">
                <Alert variant="error" title={t('chat.create_failed_title', 'Could not create a session')}>
                  <p>{createError}</p>
                </Alert>
              </div>
            )}
            {sessionLoadError && (
              <div className="mt-2 w-full max-w-md px-6">
                <Alert variant="error" title={t('chat.load_failed_title', 'Could not load sessions')}>
                  <p>{sessionLoadError}</p>
                </Alert>
              </div>
            )}
          </div>
        ) : (
          <>
            <div className="flex items-center gap-2 border-muted border-b bg-surface-base px-4 py-2">
              <span className={cn('h-2 w-2 rounded-full', STATE_DOT[active?.state ?? 'terminated'])} />
              <span className={cn('text-xs', typography.weight.medium, colors.text.primary)}>
                {active?.title || active?.model || active?.provider_name || 'Session'}
              </span>
              <span className={cn('text-[10px]', colors.text.secondary)}>({active?.transport})</span>
              {tokenUsage.total > 0 && (
                <span
                  className={cn(
                    'text-[10px]',
                    tokenUsage.budget && tokenUsage.total > tokenUsage.budget * 0.9
                      ? 'text-semantic-error'
                      : 'text-content-tertiary',
                  )}
                >
                  {tokenUsage.total.toLocaleString()} tokens
                  {tokenUsage.budget ? ` / ${tokenUsage.budget.toLocaleString()}` : ''}
                </span>
              )}
              <div className="ml-auto flex items-center gap-1">
                {searchOpen && (
                  <div className="flex items-center gap-1">
                    <input
                      type="text"
                      value={searchQuery}
                      onChange={(e) => setSearchQuery(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Escape') {
                          setSearchQuery('')
                          setSearchOpen(false)
                        }
                      }}
                      placeholder="Search messages..."
                      className={cn(
                        'w-40 border bg-surface-base px-2 py-1 text-xs placeholder-content-tertiary',
                        radius.md,
                        interaction.focusRing,
                        colors.text.primary,
                        'border-DEFAULT focus:border-brand-signal',
                      )}
                    />
                    {searchQuery && (
                      <span className={cn('whitespace-nowrap text-[10px]', colors.text.secondary)}>
                        {searchMatchCount} {searchMatchCount === 1 ? 'match' : 'matches'}
                      </span>
                    )}
                  </div>
                )}
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setSearchOpen((p) => !p)
                    if (searchOpen) setSearchQuery('')
                  }}
                >
                  <Search className={iconSize.xs} />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => handleExport('json')}
                  title={t('chat.export_json', 'Export JSON')}
                >
                  <Download className={iconSize.xs} />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => handleExport('markdown')}
                  title={t('chat.exportMarkdown', 'Export as Markdown')}
                >
                  <FileText className={iconSize.xs} />
                </Button>
                {active?.state === 'failed' && (
                  <Button variant="ghost" size="sm" onClick={handleRetry} className="text-xs">
                    <RefreshCw className={iconSize.xs} /> {t('chat.retry')}
                  </Button>
                )}
              </div>
            </div>
            <div ref={scrollRef} onScroll={handleScroll} className="flex-1 space-y-3 overflow-y-auto px-4 py-3">
              {isTruncated && (
                <button
                  type="button"
                  onClick={() => {
                    scrollRef.current?.scrollTo({ top: 0 })
                  }}
                  className={cn(
                    'mx-auto block rounded-full border px-3 py-1 text-xs',
                    'border-muted bg-surface-elevated text-content-secondary',
                    interaction.interactive,
                  )}
                >
                  {messages.length - MAX_VISIBLE_MESSAGES} earlier messages not shown
                </button>
              )}
              {visibleMessages.map((msg) => {
                const matches = searchQuery && msg.content.toLowerCase().includes(searchQuery.toLowerCase())
                const dimmed = searchQuery && !matches
                return (
                  <div key={`${msg.timestamp}-${msg.role}`} className={cn(dimmed && 'opacity-30', motion.opacity)}>
                    <MessageBubble msg={msg} onRetry={handleRetry} highlight={matches ? searchQuery : undefined} />
                  </div>
                )
              })}
              {sending &&
                messages[messages.length - 1]?.role !== 'assistant' &&
                !messages[messages.length - 1]?.thinking && (
                  <div className="flex items-center gap-2 text-content-secondary text-xs">
                    <Loader2 className={cn(iconSize.sm, 'animate-spin')} /> {t('chat.thinking')}
                  </div>
                )}
            </div>
            <button
              type="button"
              onClick={() => setShowMessagePayload((p) => !p)}
              className={cn(
                'flex items-center gap-2 border-muted border-t bg-surface-base px-4 py-2 text-xs',
                interaction.interactive,
                colors.text.secondary,
              )}
            >
              <ChevronDown className={cn(iconSize.xs, motion.transform, showMessagePayload && 'rotate-180')} />
              <span>{t('chat.message_payload')}</span>
              {messagePayloadCount > 0 && (
                <span className={cn('rounded-full bg-surface-elevated px-1.5 py-0.5 text-[10px]', colors.text.primary)}>
                  {messagePayloadCount}
                </span>
              )}
            </button>
            {showMessagePayload && (
              <div className="space-y-3 bg-surface-base px-4 py-3">
                <div className="grid gap-3 md:grid-cols-2">
                  <div className="space-y-1">
                    <p
                      className={cn(
                        'text-[10px] uppercase tracking-[0.12em]',
                        typography.weight.medium,
                        colors.text.secondary,
                      )}
                    >
                      {t('chat.regime_label')}
                    </p>
                    <Input
                      value={contextRegime}
                      onChange={(e) => setContextRegime(e.target.value)}
                      placeholder={t('chat.regime_placeholder')}
                      className="text-xs"
                    />
                  </div>
                  <div className="space-y-1">
                    <p
                      className={cn(
                        'text-[10px] uppercase tracking-[0.12em]',
                        typography.weight.medium,
                        colors.text.secondary,
                      )}
                    >
                      {t('chat.active_app_label')}
                    </p>
                    <Input
                      value={contextActiveApp}
                      onChange={(e) => setContextActiveApp(e.target.value)}
                      placeholder={t('chat.active_app_placeholder')}
                      className="text-xs"
                    />
                  </div>
                </div>
                <div className="space-y-1">
                  <p
                    className={cn(
                      'text-[10px] uppercase tracking-[0.12em]',
                      typography.weight.medium,
                      colors.text.secondary,
                    )}
                  >
                    {t('chat.tools_label')}
                  </p>
                  <textarea
                    value={toolsJson}
                    onChange={(e) => setToolsJson(e.target.value)}
                    placeholder={t('chat.tools_placeholder')}
                    rows={4}
                    className={cn(
                      'w-full resize-y border bg-surface-base px-2 py-1.5 text-xs placeholder-content-tertiary',
                      radius.md,
                      interaction.focusRing,
                      colors.text.primary,
                      'border-DEFAULT focus:border-brand-signal',
                    )}
                  />
                  {parsedTools.error && (
                    <p className="text-[10px] text-semantic-error">{t('chat.invalid_tools_json')}</p>
                  )}
                </div>
                <div className="space-y-1">
                  <p
                    className={cn(
                      'text-[10px] uppercase tracking-[0.12em]',
                      typography.weight.medium,
                      colors.text.secondary,
                    )}
                  >
                    {t('chat.response_format_label')}
                  </p>
                  <textarea
                    value={responseFormatJson}
                    onChange={(e) => setResponseFormatJson(e.target.value)}
                    placeholder={t('chat.response_format_placeholder')}
                    rows={4}
                    className={cn(
                      'w-full resize-y border bg-surface-base px-2 py-1.5 text-xs placeholder-content-tertiary',
                      radius.md,
                      interaction.focusRing,
                      colors.text.primary,
                      'border-DEFAULT focus:border-brand-signal',
                    )}
                  />
                  {parsedResponseFormat.error && (
                    <p className="text-[10px] text-semantic-error">{t('chat.invalid_response_format_json')}</p>
                  )}
                </div>
              </div>
            )}
            <ChatInput
              input={input}
              setInput={setInput}
              attachments={attachments}
              setAttachments={setAttachments}
              isReadOnly={isReadOnly}
              sending={sending}
              sendDisabled={sendDisabled}
              onSend={handleSend}
              onRequestSuggestions={activeId ? handleRequestSuggestions : undefined}
              requestingSuggestions={requestingSuggestions || suggestionCooldown}
              audioAvailable={audioAvailable}
              audioTooltip={audioTooltip}
              micMode={micMode}
              vadState={vadState}
              recording={recording}
              transcribing={transcribing}
              onMicDown={handleMicDown}
              onMicUp={handleMicUp}
              onVadToggle={handleVadToggle}
            />
            {tokenUsage.total > 0 && (
              <div
                className={cn(
                  'border-muted border-t bg-surface-base px-4 py-1 text-right text-[10px]',
                  colors.text.secondary,
                )}
              >
                {t('chat.tokensToday', 'Today: {{count}} tokens', { count: tokenUsage.total })}
                {tokenUsage.budget ? ` / ${tokenUsage.budget.toLocaleString()}` : ''}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  )
}
