import { AlertTriangle, RefreshCw } from 'lucide-react'
import { useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { Button, Spinner } from '../components/ui'
import { iconSize, radius, typography } from '../styles/tokens'

interface RouteErrorFallbackProps {
  error: Error
  route: string
  componentStack?: string
  isRecovering?: boolean
  onRetry: () => void
  onGoHome: () => void
}

export function RouteErrorFallback({
  error,
  route,
  componentStack,
  isRecovering,
  onRetry,
  onGoHome,
}: RouteErrorFallbackProps) {
  const { t } = useTranslation()
  const retryButtonRef = useRef<HTMLButtonElement | null>(null)
  const recoveringHeadingRef = useRef<HTMLHeadingElement | null>(null)

  // Move keyboard focus to the primary recovery action on mount so keyboard
  // users don't have to tab through whatever focus remained from the crashed
  // component. Pairs with `role="alert"` to announce the error via SR.
  //
  // When transitioning from error → recovering, move focus to the recovering
  // heading so the user's focus stays in the fallback container instead of
  // being dropped to <body>. The heading has tabIndex={-1} for
  // programmatic focus without affecting tab order.
  useEffect(() => {
    if (isRecovering) {
      recoveringHeadingRef.current?.focus()
    } else {
      retryButtonRef.current?.focus()
    }
  }, [isRecovering])

  // Recovering state — the user crossed the escalation threshold, Rust is
  // scheduling a full-reload, and a safety-net timeout will force reload
  // after CRITICAL_RELOAD_FALLBACK_MS if Rust doesn't emit first.
  if (isRecovering) {
    return (
      <div
        className={`flex min-h-full items-center justify-center p-8 ${radius.md}`}
        role="alert"
        aria-live="assertive"
      >
        <div className="w-full max-w-md text-center">
          <div className="mb-4 flex justify-center" aria-hidden="true">
            <div className="inline-flex items-center justify-center rounded-full bg-semantic-warning/10 p-3">
              <RefreshCw className={`${iconSize.lg} animate-spin text-semantic-warning`} />
            </div>
          </div>
          <h1
            ref={recoveringHeadingRef}
            tabIndex={-1}
            className={`mb-2 ${typography.h3} text-content focus:outline-none`}
          >
            {t('errors.route.recoveringTitle')}
          </h1>
          <p className={`mb-6 ${typography.body} text-content-secondary`}>{t('errors.route.recoveringDescription')}</p>
          <div className="flex items-center justify-center gap-3">
            <Spinner size="sm" />
            <span className={`${typography.caption} text-content-tertiary`}>{t('errors.route.recovering')}</span>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div className={`flex min-h-full items-center justify-center p-8 ${radius.md}`} role="alert" aria-live="assertive">
      <div className="w-full max-w-md text-center">
        {/* Error icon (decorative — labelled by the role="alert" heading below) */}
        <div className="mb-4 flex justify-center" aria-hidden="true">
          <div className="inline-flex items-center justify-center rounded-full bg-semantic-error/10 p-3">
            <AlertTriangle className={`${iconSize.lg} text-semantic-error`} />
          </div>
        </div>

        {/* Title and description (h1 because the crashed Layout's own h1 is gone) */}
        <h1 className={`mb-2 ${typography.h3} text-content`}>{t('errors.route.title')}</h1>
        <p className={`mb-6 ${typography.body} text-content-secondary`}>{t('errors.route.description')}</p>

        {/* Action buttons */}
        <div className="flex items-center justify-center gap-3">
          <Button ref={retryButtonRef} variant="primary" size="md" onClick={onRetry}>
            {t('errors.route.tryAgain')}
          </Button>
          <Button variant="secondary" size="md" onClick={onGoHome}>
            {t('errors.route.goHome')}
          </Button>
        </div>

        {/* Dev-only debug details — self-controlling <details>, no state needed */}
        {import.meta.env.DEV && (
          <details className="mt-6 text-left">
            <summary className={`cursor-pointer select-none ${typography.caption} text-content-tertiary`}>
              Debug: {route} — {error.name}
            </summary>
            <div className="mt-2 space-y-2">
              <pre className="overflow-auto rounded bg-surface-muted p-3 text-content-secondary text-xs">
                {error.stack ?? error.message}
              </pre>
              {componentStack && (
                <pre className="overflow-auto rounded bg-surface-muted p-3 text-content-tertiary text-xs">
                  {componentStack}
                </pre>
              )}
            </div>
          </details>
        )}
      </div>
    </div>
  )
}
