import { ChevronDown, Pencil, Plus, RefreshCw, Search, Trash2 } from 'lucide-react'
import { useCallback, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { ProviderSurfaceSpec } from '../../api/contracts'
import { Badge, Button, Input, Select } from '../../components/ui'
import { defaultSurfaceModel } from '../../features/providerSurfaces'
import { colors, iconSize, interaction, motion, radius, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { STATE_DOT } from './constants'
import type { SessionInfo, Transport } from './types'

interface ChatSidebarProps {
  sessions: SessionInfo[]
  activeId: string | null
  transport: Transport
  setTransport: (t: Transport) => void
  httpApiSurfaces: ProviderSurfaceSpec[]
  selectedHttpSurface: ProviderSurfaceSpec | null
  setHttpSurfaceId: (id: string) => void
  modelOverride: string
  setModelOverride: (v: string) => void
  systemPrompt: string
  setSystemPrompt: (v: string) => void
  showAdvanced: boolean
  setShowAdvanced: (v: boolean | ((p: boolean) => boolean)) => void
  creating: boolean
  createDisabled: boolean
  onRefresh: () => void
  onSelectSession: (id: string) => void
  onCreate: () => void
  onDelete: (id: string) => void
  onRename: (id: string, title: string) => void
  isHistorical: (s: SessionInfo) => boolean
}

export function ChatSidebar({
  sessions,
  activeId,
  transport,
  setTransport,
  httpApiSurfaces,
  selectedHttpSurface,
  setHttpSurfaceId,
  modelOverride,
  setModelOverride,
  systemPrompt,
  setSystemPrompt,
  showAdvanced,
  setShowAdvanced,
  creating,
  createDisabled,
  onRefresh,
  onSelectSession,
  onCreate,
  onDelete,
  onRename,
  isHistorical,
}: ChatSidebarProps) {
  const { t } = useTranslation()
  const [searchFilter, setSearchFilter] = useState('')
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editValue, setEditValue] = useState('')
  const renameInputRef = useRef<HTMLInputElement>(null)

  const filteredSessions = useMemo(() => {
    if (!searchFilter.trim()) return sessions
    const q = searchFilter.toLowerCase()
    return sessions.filter(
      (s) =>
        (s.title ?? s.model ?? s.provider_name).toLowerCase().includes(q) ||
        s.model?.toLowerCase().includes(q) ||
        s.provider_name?.toLowerCase().includes(q),
    )
  }, [sessions, searchFilter])

  const startRename = useCallback((s: SessionInfo) => {
    setEditingId(s.session_id)
    setEditValue(s.title ?? s.model ?? s.provider_name)
    setTimeout(() => renameInputRef.current?.select(), 0)
  }, [])

  const commitRename = useCallback(() => {
    if (editingId && editValue.trim()) {
      onRename(editingId, editValue.trim())
    }
    setEditingId(null)
  }, [editingId, editValue, onRename])

  const cancelRename = useCallback(() => {
    setEditingId(null)
  }, [])

  return (
    <div className="flex w-64 shrink-0 flex-col border-muted border-r bg-surface-base">
      <div className="flex items-center justify-between border-muted border-b px-3 py-2">
        <span className={cn(typography.label, colors.text.primary)}>{t('chat.title')}</span>
        <Button variant="ghost" size="sm" onClick={onRefresh}>
          <RefreshCw className={iconSize.sm} />
        </Button>
      </div>
      <div className="flex items-center gap-1 border-muted border-b px-2 py-2">
        <Select
          selectSize="sm"
          value={transport}
          onChange={(e) => setTransport(e.target.value as Transport)}
          className="flex-1 text-xs"
        >
          <option value="subprocess">Subprocess</option>
          <option value="http_api">HTTP API</option>
          <option value="local_llm">Local LLM</option>
        </Select>
        <Button variant="primary" size="sm" onClick={onCreate} isLoading={creating} disabled={createDisabled}>
          <Plus className={iconSize.sm} />
        </Button>
      </div>

      {/* Advanced settings toggle */}
      <button
        type="button"
        onClick={() => setShowAdvanced((p: boolean) => !p)}
        className={cn(
          'flex items-center gap-1 border-muted border-b px-3 py-1.5 text-xs',
          interaction.interactive,
          colors.text.secondary,
        )}
      >
        <ChevronDown className={cn(iconSize.xs, motion.transform, showAdvanced && 'rotate-180')} />
        {t('chat.advanced')}
      </button>
      {showAdvanced && (
        <div className="space-y-3 border-muted border-b px-2 py-2">
          {transport === 'http_api' && (
            <div className="space-y-1">
              <p
                className={cn(
                  'text-[10px] uppercase tracking-[0.12em]',
                  typography.weight.medium,
                  colors.text.secondary,
                )}
              >
                {t('chat.http_surface_label')}
              </p>
              <Select
                selectSize="sm"
                value={selectedHttpSurface?.surface_id ?? ''}
                onChange={(e) => setHttpSurfaceId(e.target.value)}
                className="w-full text-xs"
              >
                {httpApiSurfaces.map((surface) => (
                  <option key={surface.surface_id} value={surface.surface_id}>
                    {surface.display_name}
                  </option>
                ))}
              </Select>
              <p className={cn('text-[10px]', colors.text.secondary)}>{t('chat.http_surface_help')}</p>
            </div>
          )}
          <div className="space-y-1">
            <p
              className={cn('text-[10px] uppercase tracking-[0.12em]', typography.weight.medium, colors.text.secondary)}
            >
              {t('chat.model_label')}
            </p>
            <Input
              value={modelOverride}
              onChange={(e) => setModelOverride(e.target.value)}
              placeholder={
                transport === 'http_api'
                  ? (defaultSurfaceModel(selectedHttpSurface ?? undefined, 'llm_api') ?? t('chat.model_placeholder'))
                  : t('chat.model_placeholder')
              }
              className="text-xs"
            />
          </div>
          <textarea
            value={systemPrompt}
            onChange={(e) => setSystemPrompt(e.target.value)}
            placeholder={t('chat.system_prompt_placeholder')}
            rows={3}
            className={cn(
              'w-full resize-none border bg-surface-base px-2 py-1.5 text-xs placeholder-content-tertiary',
              radius.md,
              interaction.focusRing,
              colors.text.primary,
              'border-DEFAULT focus:border-brand-signal',
            )}
          />
        </div>
      )}

      {/* Search filter */}
      <div className="border-muted border-b px-2 py-1.5">
        <div className="relative">
          <Search className={cn(iconSize.xs, 'absolute top-1/2 left-2 -translate-y-1/2 text-content-muted')} />
          <input
            type="text"
            value={searchFilter}
            onChange={(e) => setSearchFilter(e.target.value)}
            placeholder={t('chat.searchSessions', 'Search sessions...')}
            className={cn(
              'w-full border bg-surface-base py-1 pr-2 pl-8 text-xs placeholder-content-tertiary',
              radius.md,
              interaction.focusRing,
              colors.text.primary,
              'border-DEFAULT focus:border-brand-signal',
            )}
          />
        </div>
      </div>

      <div className="flex-1 overflow-y-auto">
        {filteredSessions.length === 0 ? (
          <p className={cn('px-3 py-4 text-center text-xs', colors.text.secondary)}>{t('chat.no_sessions')}</p>
        ) : (
          filteredSessions.map((s) => (
            <button
              key={s.session_id}
              type="button"
              onClick={() => onSelectSession(s.session_id)}
              onDoubleClick={(e) => {
                e.preventDefault()
                startRename(s)
              }}
              className={cn(
                'group flex w-full items-center gap-2 px-3 py-2 text-left',
                interaction.interactive,
                activeId === s.session_id ? 'bg-surface-elevated' : 'hover:bg-hover',
              )}
            >
              <span className={cn('h-2 w-2 shrink-0 rounded-full', STATE_DOT[s.state] ?? 'bg-status-disconnected')} />
              <div className="min-w-0 flex-1">
                {editingId === s.session_id ? (
                  <input
                    ref={renameInputRef}
                    type="text"
                    value={editValue}
                    onChange={(e) => setEditValue(e.target.value)}
                    onBlur={commitRename}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') commitRename()
                      if (e.key === 'Escape') cancelRename()
                    }}
                    onClick={(e) => e.stopPropagation()}
                    className={cn(
                      'w-full border bg-surface-base px-1 py-0.5 text-xs',
                      radius.md,
                      interaction.focusRing,
                      colors.text.primary,
                      'border-DEFAULT focus:border-brand-signal',
                    )}
                  />
                ) : (
                  <p className={cn('truncate text-xs', typography.weight.medium, colors.text.primary)}>
                    {s.title || s.model || s.provider_name}
                  </p>
                )}
                <p className={cn('truncate text-[10px]', colors.text.secondary)}>
                  {s.transport} -- {s.turn_count} {t('chat.turns')}
                </p>
              </div>
              {isHistorical(s) ? (
                <Badge size="xs" className="bg-surface-muted text-content-secondary">
                  {t('chat.history', 'History')}
                </Badge>
              ) : null}
              <div className="hidden items-center gap-0.5 group-hover:flex">
                {editingId !== s.session_id && (
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation()
                      startRename(s)
                    }}
                    className="text-content-muted hover:text-content-primary"
                    title={t('chat.renameSession', 'Rename')}
                  >
                    <Pencil className={iconSize.xs} />
                  </button>
                )}
                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation()
                    onDelete(s.session_id)
                  }}
                  className="text-content-muted hover:text-semantic-error"
                >
                  <Trash2 className={iconSize.xs} />
                </button>
              </div>
            </button>
          ))
        )}
      </div>
    </div>
  )
}
