import { useTranslation } from 'react-i18next'
import { motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { SuggestionGuiAnchorPayload } from '../types'

interface SuggestionBadgeProps {
  count: number
  onClick: () => void
  anchor?: SuggestionGuiAnchorPayload | null
}

function anchorLabel(anchor: SuggestionGuiAnchorPayload) {
  return `${anchor.active_app} ${anchor.target_entity.label} ${anchor.target_entity.value}`.trim()
}

export function SuggestionBadge({ count, onClick, anchor }: SuggestionBadgeProps) {
  const { t } = useTranslation()
  if (count === 0) return null

  if (anchor) {
    const bounds = anchor.highlight.bounds
    return (
      <div
        data-testid="suggestion-contour-marker"
        className="pointer-events-none fixed z-50 rounded-lg border-2 border-brand/80 bg-brand/5 shadow-[0_0_0_3px_rgba(20,184,166,0.12)]"
        style={{
          left: bounds.x,
          top: bounds.y,
          width: bounds.width,
          height: bounds.height,
        }}
      >
        <button
          type="button"
          onClick={onClick}
          aria-live="polite"
          aria-atomic="true"
          aria-label={t('suggestions.targetBadgeLabel', 'new suggestions for {{target}}', {
            target: anchorLabel(anchor),
          })}
          className={cn(
            'pointer-events-auto absolute -top-3 -right-3 flex h-6 min-w-6 items-center justify-center rounded-full px-1.5',
            `bg-brand text-[11px] text-content-inverse ${typography.weight.semibold}`,
            'shadow-lg ring-2 ring-surface',
            `hover:bg-brand/90 ${motion.colors}`,
          )}
        >
          {count}
        </button>
      </div>
    )
  }

  return (
    <button
      type="button"
      onClick={onClick}
      aria-live="polite"
      aria-atomic="true"
      aria-label={t('suggestions.badgeLabel', '{{count}} new suggestions', { count })}
      className={cn(
        'fixed top-4 right-4 z-50',
        'flex items-center gap-1.5 rounded-full px-3 py-1.5',
        `bg-brand text-content-inverse text-xs ${typography.weight.medium}`,
        'cursor-pointer shadow-lg',
        `hover:bg-brand/90 ${motion.colors}`,
        'animate-pulse',
      )}
    >
      <span>{count}</span>
      <span>{t('suggestions.badgeNew', 'new')}</span>
    </button>
  )
}
