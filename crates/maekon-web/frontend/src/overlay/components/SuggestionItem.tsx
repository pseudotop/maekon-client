import { Lock, Play, ShieldCheck } from 'lucide-react'
import { memo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { translateError, type WireErrorLocale } from '../../i18n/translateError'
import { iconSize, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { SuggestionViewDto } from '../types'
import { SnoozePopover } from './SnoozePopover'
import { showToast } from './Toast'

interface SuggestionItemProps {
  item: SuggestionViewDto
  onAction: (id: string, action: 'accept' | 'reject' | 'defer' | 'explain', snoozeMinutes?: number) => void
  /** Called after a successful one-click run so the parent can refresh the list. */
  onRan?: () => void | Promise<void>
}

const priorityClasses: Record<string, string> = {
  critical: 'bg-semantic-error/20 text-semantic-error',
  high: 'bg-semantic-warning/20 text-semantic-warning',
  medium: 'bg-surface-muted text-content-secondary',
  low: 'bg-surface-muted text-content-tertiary',
}

const secondaryActionClass =
  'inline-flex min-w-0 items-center justify-center whitespace-nowrap rounded-md border border-muted bg-surface-muted px-2 py-1.5 text-content-secondary text-xs hover:bg-active hover:text-content'

const primaryActionClass =
  'inline-flex min-w-0 items-center justify-center whitespace-nowrap rounded-md bg-brand px-2 py-1.5 text-content-inverse text-xs hover:bg-brand-hover'

export const SuggestionItem = memo(function SuggestionItem({ item, onAction, onRan }: SuggestionItemProps) {
  const { t, i18n } = useTranslation()
  const [showSnooze, setShowSnooze] = useState(false)
  const [running, setRunning] = useState(false)
  const badgeClass = priorityClasses[item.priority] ?? priorityClasses.low
  const requiresClarification = item.category === 'clarification-required'
  // #7917: present only on bound, live pending items (never on history/unbound).
  const action = item.action ?? null
  // Mirror SuggestionsPanel's wire-error localization (ADR-019 Follow-up #3).
  const errorLocale: WireErrorLocale = i18n.language?.startsWith('ko') ? 'ko' : 'en'
  const sourceLabel =
    item.source === 'server'
      ? t('suggestions.sourceServer')
      : item.source === 'local'
        ? t('suggestions.sourceLocal')
        : item.source === 'rule'
          ? t('suggestions.sourceRules')
          : item.source
  const createdAt = new Date(item.created_at)
  const createdAtLabel = Number.isNaN(createdAt.getTime())
    ? item.created_at
    : new Intl.DateTimeFormat(i18n.language, {
        dateStyle: 'medium',
        timeStyle: 'short',
      }).format(createdAt)

  async function handleRun() {
    if (running || !action) return
    setRunning(true)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      // The command re-derives the preset server-side from the suggestion id —
      // the client never supplies a preset id (ADR-027).
      await invoke('run_suggestion_action', { suggestionId: item.id })
      showToast(t('suggestions.toastActionRan', 'Action ran'), 'success')
      await Promise.resolve(onRan?.())
    } catch (e) {
      // Distinguishes policy-denied / disabled / error via the localized wire code.
      showToast(`${t('suggestions.actionFailed', 'Could not run action:')} ${translateError(e, errorLocale)}`, 'error')
    } finally {
      setRunning(false)
    }
  }

  return (
    <li
      aria-label={t('suggestions.suggestionLabel', { title: item.title })}
      className="list-none border-content-inverse/5 border-b px-4 py-4"
    >
      <div className="flex items-start justify-between gap-2">
        <span className={cn('text-content text-sm leading-tight', typography.weight.medium)}>{item.title}</span>
        <span
          className={cn(
            'inline-flex shrink-0 items-center rounded px-1.5 py-0.5 text-[10px]',
            typography.weight.semibold,
            badgeClass,
          )}
        >
          {item.priority}
        </span>
      </div>
      <p className="mt-1 line-clamp-2 text-content-secondary text-xs">{item.body}</p>
      {action ? (
        // Bound item: policy-neutral gate copy — never promises a prompt, since
        // the user's confirmation policy (Auto/Confirm/Block) governs the run.
        <div className="mt-2 flex items-start gap-1.5 text-[11px] text-content-tertiary leading-4">
          <ShieldCheck className={cn(iconSize.xs, 'mt-0.5 shrink-0 text-content-secondary')} />
          <span className="min-w-0">
            {t('suggestions.gateNotice', 'Runs through the automation gate — your confirmation settings apply.')}
          </span>
        </div>
      ) : (
        <div className="mt-2 flex items-center gap-1.5 text-[11px] text-content-tertiary">
          <Lock className={cn(iconSize.xs, 'shrink-0')} />
          <span>{t('suggestions.noAutoAction', 'No auto action')}</span>
        </div>
      )}
      <div className="mt-3 space-y-1.5">
        {action && (
          <button
            type="button"
            data-testid="run-suggestion-action"
            disabled={running}
            aria-busy={running}
            onClick={handleRun}
            className={cn(
              primaryActionClass,
              'w-full gap-1.5 disabled:cursor-not-allowed disabled:opacity-60',
              motion.colors,
            )}
          >
            <Play className={cn(iconSize.xs, 'shrink-0')} />
            <span className="truncate">
              {running
                ? t('suggestions.runActionPending', 'Running…')
                : t('suggestions.runAction', 'Run {{label}}', { label: action.label })}
            </span>
          </button>
        )}
        <div data-testid="suggestion-review-actions" className="grid grid-cols-2 gap-1.5">
          {!requiresClarification && (
            <button
              type="button"
              onClick={() => onAction(item.id, 'accept')}
              className={cn(action ? secondaryActionClass : primaryActionClass, motion.colors)}
            >
              {t('suggestions.accept')}
            </button>
          )}
          <button
            type="button"
            onClick={() => onAction(item.id, 'reject')}
            className={cn(secondaryActionClass, motion.colors)}
          >
            {t('suggestions.reject')}
          </button>
          {!requiresClarification && (
            <div className="relative min-w-0">
              <button
                type="button"
                onClick={() => setShowSnooze(!showSnooze)}
                className={cn(secondaryActionClass, 'w-full', motion.colors)}
              >
                {t('suggestions.later')}
              </button>
              {showSnooze && (
                <SnoozePopover
                  onSelect={(minutes) => {
                    onAction(item.id, 'defer', minutes)
                    setShowSnooze(false)
                  }}
                  onCancel={() => setShowSnooze(false)}
                />
              )}
            </div>
          )}
          <button
            type="button"
            onClick={() => onAction(item.id, 'explain')}
            className={cn(requiresClarification ? primaryActionClass : secondaryActionClass, motion.colors)}
          >
            {t('suggestions.explain')}
          </button>
        </div>
      </div>
      <div className="mt-2 flex justify-end text-[10px] text-content-tertiary">
        <span>
          {Math.round(item.confidence_score * 100)}% &middot; {sourceLabel} &middot;{' '}
          <time data-testid="suggestion-created-at" dateTime={item.created_at} title={createdAtLabel}>
            {createdAtLabel}
          </time>
        </span>
      </div>
    </li>
  )
})
