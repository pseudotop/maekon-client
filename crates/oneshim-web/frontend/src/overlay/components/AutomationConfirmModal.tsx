import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { PendingConfirmationDto } from '../types'

const AUTO_DENY_SECS = 30

/** Strip Unicode control characters that could disguise the actual command. */
// biome-ignore lint/suspicious/noControlCharactersInRegex: intentional — stripping C0/C1 control chars, DEL, zero-width chars, bidi overrides, line/paragraph separators, BOM
const CONTROL_CHAR_RE = /[\u0000-\u001F\u007F-\u009F\u200B-\u200F\u2028-\u202F\uFEFF]/g

function sanitizeArg(arg: string): string {
  return arg.replace(CONTROL_CHAR_RE, '')
}

const auditBadgeColors: Record<string, string> = {
  Critical: 'bg-semantic-error/20 text-semantic-error',
  Elevated: 'bg-semantic-warning/20 text-semantic-warning',
  Basic: 'bg-semantic-info/20 text-semantic-info',
}

interface AutomationConfirmModalProps {
  confirmation: PendingConfirmationDto
  onDismiss: () => void
}

export function AutomationConfirmModal({ confirmation, onDismiss }: AutomationConfirmModalProps) {
  const { t } = useTranslation()
  const [remaining, setRemaining] = useState(AUTO_DENY_SECS)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  // Countdown timer — resets when a new confirmation arrives (keyed by command_id)
  const commandId = confirmation.command_id
  useEffect(() => {
    // commandId is read here to satisfy exhaustive-deps while acting as a reset trigger
    void commandId
    setRemaining(AUTO_DENY_SECS)
    intervalRef.current = setInterval(() => {
      setRemaining((prev) => {
        if (prev <= 1) {
          if (intervalRef.current) clearInterval(intervalRef.current)
          return 0
        }
        return prev - 1
      })
    }, 1000)
    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current)
    }
  }, [commandId])

  const handleSubmit = useCallback(
    async (approved: boolean) => {
      if (submitting) return
      setSubmitting(true)
      setError(null)
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('confirm_automation_command', {
          commandId: confirmation.command_id,
          nonce: confirmation.nonce,
          approved,
        })
        onDismiss()
      } catch (e) {
        console.warn('confirm_automation_command failed:', e)
        setError(t('automation.confirmSubmitError', 'Could not submit confirmation.'))
        setSubmitting(false)
      }
    },
    [confirmation.command_id, confirmation.nonce, onDismiss, submitting, t],
  )

  // Auto-deny on timeout
  useEffect(() => {
    if (remaining === 0 && !submitting) {
      void handleSubmit(false)
    }
  }, [remaining, handleSubmit, submitting])

  const progressPct = (remaining / AUTO_DENY_SECS) * 100
  const badgeColor = auditBadgeColors[confirmation.audit_level] ?? 'bg-content-inverse/10 text-content-secondary'

  return (
    <div className="fixed inset-0 z-overlay flex items-center justify-center">
      {/* Backdrop */}
      <div className="absolute inset-0 bg-content-inverse/30 backdrop-blur-sm" />

      {/* Modal */}
      <div className="relative w-96 max-w-[calc(100vw-2rem)] rounded-xl border border-content-inverse/10 bg-surface-sunken/95 p-6 shadow-2xl backdrop-blur-md">
        {/* Header */}
        <div className="mb-3 flex items-center justify-between">
          <h3 className={cn(typography.h4, 'text-content')}>
            {t('automation.confirmTitle', 'Automation Confirmation')}
          </h3>
          <span className={cn('rounded-full px-2 py-0.5 text-[10px]', typography.weight.semibold, badgeColor)}>
            {confirmation.audit_level}
          </span>
        </div>

        {/* Process info */}
        <div className="mb-3 rounded-lg bg-content-inverse/5 p-3">
          <div className="mb-1.5 flex items-center gap-2">
            <span className={cn(typography.caption, 'text-content-tertiary')}>
              {t('automation.confirmProcess', 'Process')}
            </span>
            <span className={cn(typography.label, 'text-content')}>{confirmation.process_name}</span>
          </div>
          {confirmation.args.length > 0 && (
            <div className="flex items-start gap-2">
              <span className={cn(typography.caption, 'shrink-0 pt-0.5 text-content-tertiary')}>
                {t('automation.confirmArgs', 'Args')}
              </span>
              <code className={cn('break-all text-[11px] text-content-secondary', typography.family.mono)}>
                {confirmation.args.map(sanitizeArg).join(' ')}
              </code>
            </div>
          )}
        </div>

        {/* Error */}
        {error && <p className="mb-3 text-semantic-error text-xs">{error}</p>}

        {/* Countdown progress */}
        <div className="mb-4 h-1 w-full overflow-hidden rounded-full bg-content-inverse/10">
          <div
            className={cn('h-full rounded-full', motion.all, remaining <= 5 ? 'bg-semantic-error' : 'bg-brand')}
            style={{ width: `${progressPct}%` }}
          />
        </div>

        {/* Actions */}
        <div className="flex items-center justify-between">
          <span className={cn(typography.caption, 'text-content-tertiary')}>
            {t('automation.confirmAutoDeny', 'Auto-deny in {{seconds}}s', { seconds: remaining })}
          </span>
          <div className="flex gap-2">
            <button
              type="button"
              disabled={submitting}
              onClick={() => void handleSubmit(false)}
              className={cn(
                'rounded-lg px-4 py-1.5 text-xs',
                typography.weight.medium,
                motion.colors,
                'bg-semantic-error/15 text-semantic-error hover:bg-semantic-error/25',
                'disabled:cursor-not-allowed disabled:opacity-50',
              )}
            >
              {t('automation.confirmDeny', 'Deny')}
            </button>
            <button
              type="button"
              disabled={submitting}
              onClick={() => void handleSubmit(true)}
              className={cn(
                'rounded-lg px-4 py-1.5 text-xs',
                typography.weight.medium,
                motion.colors,
                'bg-semantic-success/15 text-semantic-success hover:bg-semantic-success/25',
                'disabled:cursor-not-allowed disabled:opacity-50',
              )}
            >
              {t('automation.confirmApprove', 'Approve')}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
