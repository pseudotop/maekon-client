import { useTranslation } from 'react-i18next'
import { getShortcutsList } from '../../hooks/useKeyboardShortcuts'
import { iconSize, interaction, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { Dialog, DialogContent, DialogTitle } from '../ui'

interface ShortcutsHelpProps {
  onClose: () => void
}

export default function ShortcutsHelp({ onClose }: ShortcutsHelpProps) {
  const { t } = useTranslation()
  const shortcuts = getShortcutsList()

  return (
    <Dialog open onClose={onClose}>
      <DialogContent size="sm" className="mx-4">
        <div className="flex items-center justify-between border-muted border-b p-4">
          <DialogTitle>{t('shortcuts.title')}</DialogTitle>
          <button
            type="button"
            onClick={onClose}
            className={cn('rounded p-1 text-content-muted hover:text-content', interaction.focusRing)}
            aria-label={t('common.close', 'Close')}
          >
            <svg className={iconSize.md} fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <div className="space-y-2 p-4">
          {shortcuts.map(({ key, descriptionKey }) => (
            <div key={key} className="flex items-center justify-between py-1">
              <span className="text-content-secondary">{t(descriptionKey)}</span>
              <kbd
                className={cn(
                  'rounded border border-DEFAULT bg-surface-elevated px-2 py-1 text-content',
                  typography.mono,
                )}
              >
                {key}
              </kbd>
            </div>
          ))}
        </div>

        <div className="rounded-b-lg bg-surface-muted px-4 py-3 text-center">
          <span className="text-content-secondary text-sm">{t('shortcuts.closeHint')}</span>
        </div>
      </DialogContent>
    </Dialog>
  )
}

ShortcutsHelp.displayName = 'ShortcutsHelp'
