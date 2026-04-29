import { useCallback, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { iconSize, motion } from '../styles/tokens'

interface LightboxProps {
  imageUrl: string
  alt?: string
  onClose: () => void
  onPrev?: () => void
  onNext?: () => void
  hasPrev?: boolean
  hasNext?: boolean
}

export default function Lightbox({
  imageUrl,
  alt = '',
  onClose,
  onPrev,
  onNext,
  hasPrev = false,
  hasNext = false,
}: LightboxProps) {
  const { t } = useTranslation()
  const handleKeyDown = useCallback(
    (event: KeyboardEvent) => {
      switch (event.key) {
        case 'Escape':
          onClose()
          break
        case 'ArrowLeft':
          if (hasPrev && onPrev) {
            event.preventDefault()
            onPrev()
          }
          break
        case 'ArrowRight':
          if (hasNext && onNext) {
            event.preventDefault()
            onNext()
          }
          break
      }
    },
    [onClose, onPrev, onNext, hasPrev, hasNext],
  )

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown)
    document.body.style.overflow = 'hidden'

    return () => {
      window.removeEventListener('keydown', handleKeyDown)
      document.body.style.overflow = ''
    }
  }, [handleKeyDown])

  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: keyboard handling via global keydown listener (Escape, ArrowLeft, ArrowRight)
    <div
      role="dialog"
      aria-modal="true"
      aria-label={alt || t('common.lightbox', 'Image lightbox')}
      className="fixed inset-0 z-overlay flex items-center justify-center bg-surface-overlay/90"
      onClick={onClose}
    >
      {/* UI note */}
      <button
        type="button"
        onClick={onClose}
        className={`absolute top-4 right-4 z-10 p-2 text-content-inverse/70 ${motion.colors} hover:text-content-inverse`}
        aria-label={t('common.close', 'Close')}
      >
        <svg className={iconSize.hero} fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
        </svg>
      </button>

      {/* UI note */}
      {hasPrev && onPrev && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            onPrev()
          }}
          className={`absolute top-1/2 left-4 z-10 -translate-y-1/2 rounded-full bg-surface-overlay/30 p-3 text-content-inverse/70 ${motion.all} hover:bg-surface-overlay/50 hover:text-content-inverse`}
          aria-label={t('common.previous', 'Previous')}
        >
          <svg className={iconSize.hero} fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
          </svg>
        </button>
      )}

      {/* UI note */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: onClick only prevents bubble to backdrop, not interactive */}
      <img
        src={imageUrl}
        alt={alt}
        className="max-h-[90vh] max-w-[90vw] object-contain"
        onClick={(e) => e.stopPropagation()}
      />

      {/* UI note */}
      {hasNext && onNext && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            onNext()
          }}
          className={`absolute top-1/2 right-4 z-10 -translate-y-1/2 rounded-full bg-surface-overlay/30 p-3 text-content-inverse/70 ${motion.all} hover:bg-surface-overlay/50 hover:text-content-inverse`}
          aria-label={t('common.next', 'Next')}
        >
          <svg className={iconSize.hero} fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
          </svg>
        </button>
      )}

      {/* UI note */}
      <div className="absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-4 text-content-inverse/50 text-sm">
        <span>ESC {t('common.close', 'Close')}</span>
        {(hasPrev || hasNext) && <span>← → {t('common.navigate', 'Navigate')}</span>}
      </div>
    </div>
  )
}
