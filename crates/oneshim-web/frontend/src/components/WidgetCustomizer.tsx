import { Settings2 } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { colors, iconSize, motion, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import type { SectionId } from './widget-registry'
import { getWidgetsForSection } from './widget-registry'

interface WidgetCustomizerProps {
  section: SectionId
  isVisible: (id: string) => boolean
  canToggle: (id: string) => boolean
  onToggle: (id: string) => void
  onReset: () => void
}

const sectionLabelKey: Record<SectionId, string> = {
  overview: 'overview',
  monitoring: 'systemMetrics',
  insights: 'activityHeatmap',
}

export default function WidgetCustomizer({ section, isVisible, canToggle, onToggle, onReset }: WidgetCustomizerProps) {
  const { t } = useTranslation()
  const [isOpen, setIsOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)
  const widgets = getWidgetsForSection(section)

  const close = useCallback(() => setIsOpen(false), [])

  // Click-outside dismiss (mousedown, matching SegmentContextMenu)
  useEffect(() => {
    if (!isOpen) return
    const handleClickOutside = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        close()
      }
    }
    document.addEventListener('mousedown', handleClickOutside)
    return () => document.removeEventListener('mousedown', handleClickOutside)
  }, [isOpen, close])

  // Escape key dismiss
  useEffect(() => {
    if (!isOpen) return
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close()
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [isOpen, close])

  // Close when focus leaves the container (Tab-out)
  useEffect(() => {
    if (!isOpen) return
    const handleFocusOut = (e: FocusEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.relatedTarget as Node)) {
        close()
      }
    }
    containerRef.current?.addEventListener('focusout', handleFocusOut)
    const el = containerRef.current
    return () => el?.removeEventListener('focusout', handleFocusOut)
  }, [isOpen, close])

  return (
    <div ref={containerRef} className="relative">
      <button
        type="button"
        onClick={() => setIsOpen((prev) => !prev)}
        className={cn(
          'rounded-md p-1.5',
          motion.colors,
          'hover:bg-surface-muted',
          colors.text.secondary,
          isOpen && 'bg-surface-muted',
        )}
        aria-label={t('widgets.customize')}
        aria-expanded={isOpen}
        aria-haspopup="menu"
      >
        <Settings2 className={iconSize.sm} />
      </button>

      {isOpen && (
        <div
          role="menu"
          aria-label={t('widgets.title')}
          className={cn(
            'absolute top-full right-0 z-dropdown mt-1 min-w-56 rounded-lg border border-DEFAULT',
            'bg-surface-elevated shadow-lg',
            'py-1',
          )}
        >
          <div className={cn('px-3 py-2 text-xs', typography.weight.medium, colors.text.tertiary)}>
            {t(`sidebar.${sectionLabelKey[section]}`)}
          </div>

          {widgets.map((w) => {
            const visible = isVisible(w.id)
            const disabled = !canToggle(w.id)
            return (
              <button
                key={w.id}
                type="button"
                role="menuitemcheckbox"
                aria-checked={visible}
                disabled={disabled}
                onClick={() => onToggle(w.id)}
                className={cn(
                  'flex w-full items-center gap-3 px-3 py-2 text-left text-sm',
                  motion.colors,
                  disabled ? 'cursor-not-allowed opacity-50' : 'cursor-pointer hover:bg-surface-muted',
                )}
                title={disabled ? t('widgets.minOneRequired') : undefined}
              >
                <span
                  className={cn(
                    `flex ${iconSize.base} shrink-0 items-center justify-center rounded border`,
                    visible
                      ? 'border-brand-signal bg-brand-signal text-content-inverse'
                      : 'border-DEFAULT bg-surface-base',
                  )}
                >
                  {visible && (
                    <svg
                      viewBox="0 0 12 12"
                      className={iconSize.xs}
                      fill="none"
                      stroke="currentColor"
                      strokeWidth={2}
                      aria-hidden="true"
                    >
                      <path d="M2 6l3 3 5-5" />
                    </svg>
                  )}
                </span>
                <w.icon className={cn(iconSize.sm, colors.text.secondary)} />
                <span className={colors.text.primary}>{t(w.labelKey)}</span>
              </button>
            )
          })}

          <div className="my-1 border-DEFAULT border-t" />

          <button
            type="button"
            onClick={() => {
              onReset()
              close()
            }}
            className={cn(
              'w-full px-3 py-2 text-left text-xs',
              motion.colors,
              'cursor-pointer hover:bg-surface-muted',
              colors.text.secondary,
            )}
          >
            {t('widgets.resetDefaults')}
          </button>
        </div>
      )}
    </div>
  )
}
