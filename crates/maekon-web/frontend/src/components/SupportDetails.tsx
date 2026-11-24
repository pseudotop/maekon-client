import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { interaction, radius, typography } from '../styles/tokens'
import { cn } from '../utils/cn'

interface SupportDetailsProps {
  /** Untranslated code/message text. Nothing renders when it is empty. */
  detail: string | null | undefined
  className?: string
  /** Called when the user opens or closes the panel. */
  onToggle?: (open: boolean) => void
}

const COPIED_RESET_MS = 2000

/**
 * Collapsed "Details for support" panel (#13737).
 *
 * Screens show a localized sentence and keep the raw code or English
 * diagnostic here, closed by default, so it is available to whoever is helping
 * the user without being the first thing a Korean user reads. The copy button
 * hands the exact text to the clipboard for a bug report.
 */
export function SupportDetails({ detail, className, onToggle }: SupportDetailsProps) {
  const { t } = useTranslation()
  const [copied, setCopied] = useState(false)
  const resetRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(
    () => () => {
      if (resetRef.current) clearTimeout(resetRef.current)
    },
    [],
  )

  const text = detail?.trim()
  if (!text) return null

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
      if (resetRef.current) clearTimeout(resetRef.current)
      resetRef.current = setTimeout(() => setCopied(false), COPIED_RESET_MS)
    } catch {
      setCopied(false)
    }
  }

  return (
    <details
      className={cn('mt-2 text-content-secondary', typography.caption, className)}
      data-testid="support-details"
      onToggle={(event) => onToggle?.(event.currentTarget.open)}
    >
      <summary className={cn('cursor-pointer select-none', interaction.focusRing)}>
        {t('errors.supportDetails')}
      </summary>
      <code
        className={cn('mt-1 block whitespace-pre-wrap break-words bg-surface-secondary p-2', radius.sm)}
        data-testid="support-details-text"
      >
        {text}
      </code>
      <button
        type="button"
        onClick={copy}
        className={cn(
          'mt-1 border-0 bg-transparent p-0 text-accent underline underline-offset-2',
          interaction.focusRing,
        )}
      >
        {copied ? t('errors.supportDetailsCopied') : t('errors.copySupportDetails')}
      </button>
    </details>
  )
}
