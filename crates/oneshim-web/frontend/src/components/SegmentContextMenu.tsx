/**
 * SegmentContextMenu — dropdown context menu for segment override actions.
 * Positioned relative to the trigger button. Closes on click-outside or Escape.
 */
import { useCallback, useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { colors, motion, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { Divider } from './ui'

interface RegimeOption {
  id: string
  label: string
}

interface SegmentContextMenuProps {
  segmentId: string
  currentRegimeId: string
  regimeOptions: RegimeOption[]
  onMarkAsNoise: (segmentId: string) => void
  onReassignRegime: (segmentId: string, targetRegimeId: string) => void
  onClose: () => void
}

export default function SegmentContextMenu({
  segmentId,
  currentRegimeId,
  regimeOptions,
  onMarkAsNoise,
  onReassignRegime,
  onClose,
}: SegmentContextMenuProps) {
  const { t } = useTranslation()
  const menuRef = useRef<HTMLDivElement>(null)

  const handleClickOutside = useCallback(
    (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose()
      }
    },
    [onClose],
  )

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose()
      }
    },
    [onClose],
  )

  useEffect(() => {
    document.addEventListener('mousedown', handleClickOutside)
    document.addEventListener('keydown', handleKeyDown)
    return () => {
      document.removeEventListener('mousedown', handleClickOutside)
      document.removeEventListener('keydown', handleKeyDown)
    }
  }, [handleClickOutside, handleKeyDown])

  // Focus first item on mount
  useEffect(() => {
    const firstItem = menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')
    firstItem?.focus()
  }, [])

  const handleMenuKeyDown = (e: React.KeyboardEvent) => {
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [])
    const idx = items.indexOf(e.target as HTMLButtonElement)
    if (idx === -1) return

    if (e.key === 'ArrowDown') {
      e.preventDefault()
      items[(idx + 1) % items.length]?.focus()
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      items[(idx - 1 + items.length) % items.length]?.focus()
    }
  }

  const filteredRegimes = regimeOptions.filter((r) => r.id !== currentRegimeId)

  return (
    <div
      ref={menuRef}
      role="menu"
      aria-label={t('recalibration.changeRegimeTo')}
      className="absolute top-full right-0 z-dropdown mt-1 min-w-48 rounded-lg border border-DEFAULT bg-surface-elevated shadow-lg"
      onKeyDown={handleMenuKeyDown}
    >
      <div className="py-1">
        {/* Mark as personal time */}
        <button
          type="button"
          role="menuitem"
          tabIndex={-1}
          className={cn(
            `flex w-full items-center gap-2 px-3 py-2 text-left text-sm ${motion.colors} hover:bg-surface-muted`,
            colors.text.primary,
          )}
          onClick={() => {
            onMarkAsNoise(segmentId)
            onClose()
          }}
        >
          {t('recalibration.markAsPersonalTime')}
        </button>

        {/* Separator */}
        {filteredRegimes.length > 0 && <Divider className="my-1" />}

        {/* Change regime sub-items */}
        {filteredRegimes.length > 0 && (
          <div className="px-3 py-1">
            <span className={cn(typography.weight.medium, 'text-xs', colors.text.tertiary)}>
              {t('recalibration.changeRegimeTo')}
            </span>
          </div>
        )}
        {filteredRegimes.map((regime) => (
          <button
            key={regime.id}
            type="button"
            role="menuitem"
            tabIndex={-1}
            className={cn(
              `flex w-full items-center gap-2 px-3 py-2 text-left text-sm ${motion.colors} hover:bg-surface-muted`,
              colors.text.primary,
            )}
            onClick={() => {
              onReassignRegime(segmentId, regime.id)
              onClose()
            }}
          >
            {regime.label}
          </button>
        ))}
      </div>
    </div>
  )
}

SegmentContextMenu.displayName = 'SegmentContextMenu'
