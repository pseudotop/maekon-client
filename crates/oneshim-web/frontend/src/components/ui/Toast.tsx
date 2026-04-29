import { AlertTriangle, CheckCircle2, Info, X, XCircle } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { removeToast, type Toast as ToastRecord, type ToastType, useToast } from '../../hooks/useToast'
import { iconSize, interaction, radius, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

const iconByType: Record<ToastType, typeof CheckCircle2> = {
  success: CheckCircle2,
  error: XCircle,
  info: Info,
  warning: AlertTriangle,
}

const iconToneByType: Record<ToastType, string> = {
  success: 'text-semantic-success bg-semantic-success/20',
  error: 'text-semantic-error bg-semantic-error/20',
  info: 'text-semantic-info bg-semantic-info/20',
  warning: 'text-semantic-warning bg-semantic-warning/20',
}

function ToastItem({ toast }: { toast: ToastRecord }) {
  const { t } = useTranslation()
  const Icon = iconByType[toast.type]

  return (
    <div
      className={cn(
        'pointer-events-auto flex w-full items-start gap-3 border border-border/70 bg-surface-overlay/95 px-4 py-3 shadow-[0_14px_36px_rgb(var(--content)/0.16)] backdrop-blur-sm',
        radius.lg,
        'animate-toast-in',
      )}
    >
      <div
        className={cn(
          'mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-full',
          iconToneByType[toast.type],
        )}
      >
        <Icon className={iconSize.base} aria-hidden="true" />
      </div>

      <div className="min-w-0 flex-1">
        <p className={cn(typography.body, 'text-content')}>{toast.message}</p>
      </div>

      <button
        type="button"
        onClick={() => removeToast(toast.id)}
        className={cn(
          'mt-0.5 inline-flex h-8 w-8 shrink-0 items-center justify-center text-content-secondary hover:bg-surface-elevated hover:text-content',
          radius.md,
          interaction.focusRing,
        )}
        aria-label={t('common.close')}
      >
        <X className={iconSize.base} aria-hidden="true" />
      </button>
    </div>
  )
}

export function ToastContainer() {
  const { toasts } = useToast()

  if (toasts.length === 0) {
    return null
  }

  return (
    <div
      className="pointer-events-none fixed right-4 bottom-10 z-toast flex w-[min(26rem,calc(100vw-2rem))] flex-col gap-2"
      aria-live="polite"
      aria-atomic="false"
    >
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
    </div>
  )
}
