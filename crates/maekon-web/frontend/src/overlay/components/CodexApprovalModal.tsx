import { useCallback, useEffect, useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { sanitizeControlChars } from '../sanitize'
import type { CodexApprovalDto } from '../types'

const AUTO_DECLINE_SECS = 30

/** The user's verdict — mirrors the `respond_codex_approval` decision strings. */
type CodexDecision = 'accept' | 'decline' | 'cancel'

interface CodexApprovalModalProps {
  approval: CodexApprovalDto
  onDismiss: () => void
}

/**
 * Codex `app-server` approval modal (E21 #5044, FU-A of #4870). Renders a
 * pending command / file-change approval and lets the user accept / decline /
 * cancel. FAIL-CLOSED: Escape, the 30s auto-timeout, and the explicit Cancel
 * button all resolve to a `decline` (only the Accept button approves).
 *
 * Separate from `AutomationConfirmModal` — different backend path
 * (`respond_codex_approval` vs `confirm_automation_command`).
 */
export function CodexApprovalModal({ approval, onDismiss }: CodexApprovalModalProps) {
  const { t } = useTranslation()
  const [remaining, setRemaining] = useState(AUTO_DECLINE_SECS)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const titleId = useId()
  const descId = useId()
  // Initial focus on the safest action (Decline).
  const declineButtonRef = useRef<HTMLButtonElement | null>(null)

  const requestId = approval.requestId
  useEffect(() => {
    void requestId
    setRemaining(AUTO_DECLINE_SECS)
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
  }, [requestId])

  const handleSubmit = useCallback(
    async (decision: CodexDecision) => {
      if (submitting) return
      setSubmitting(true)
      setError(null)
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('respond_codex_approval', {
          requestId: approval.requestId,
          decision,
        })
        onDismiss()
      } catch (e) {
        console.warn('respond_codex_approval failed:', e)
        setError(t('codex.approvalSubmitError', 'Could not submit the approval.'))
        setSubmitting(false)
      }
    },
    [approval.requestId, onDismiss, submitting, t],
  )

  // Auto-decline on timeout (fail-closed).
  useEffect(() => {
    if (remaining === 0 && !submitting) {
      void handleSubmit('decline')
    }
  }, [remaining, handleSubmit, submitting])

  useEffect(() => {
    void requestId
    declineButtonRef.current?.focus()
  }, [requestId])

  // Escape → explicit decline (safest action; fail-closed).
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        void handleSubmit('decline')
      }
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [handleSubmit])

  const progressPct = (remaining / AUTO_DECLINE_SECS) * 100
  const isFileChange = approval.kind === 'file_change'

  return (
    <div className="fixed inset-0 z-overlay flex items-center justify-center">
      {/* Backdrop */}
      <div className="absolute inset-0 bg-content-inverse/30 backdrop-blur-sm" />

      {/* Modal */}
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descId}
        className="relative w-96 max-w-[calc(100vw-2rem)] rounded-xl border border-content-inverse/10 bg-surface-sunken/95 p-6 shadow-2xl backdrop-blur-md"
      >
        {/* Header */}
        <div className="mb-3 flex items-center justify-between">
          <h3 id={titleId} className={cn(typography.h4, 'text-content')}>
            {t('codex.approvalTitle', 'Codex Approval')}
          </h3>
          <span
            className={cn(
              'rounded-full px-2 py-0.5 text-[10px]',
              typography.weight.semibold,
              'bg-semantic-info/20 text-semantic-info',
            )}
          >
            {sanitizeControlChars(approval.kind)}
          </span>
        </div>

        {/* Context */}
        <div id={descId} className="mb-3 rounded-lg bg-content-inverse/5 p-3">
          <p className={cn(typography.label, 'mb-1.5 text-content')}>{sanitizeControlChars(approval.summary)}</p>

          {!isFileChange && approval.processName && (
            <div className="mb-1.5 flex items-center gap-2">
              <span className={cn(typography.caption, 'text-content-tertiary')}>{t('codex.process', 'Process')}</span>
              <span className={cn(typography.label, 'text-content')}>{sanitizeControlChars(approval.processName)}</span>
            </div>
          )}
          {!isFileChange && approval.args.length > 0 && (
            <div className="flex items-start gap-2">
              <span className={cn(typography.caption, 'shrink-0 pt-0.5 text-content-tertiary')}>
                {t('codex.args', 'Args')}
              </span>
              <code className={cn('break-all text-[11px] text-content-secondary', typography.family.mono)}>
                {approval.args.map(sanitizeControlChars).join(' ')}
              </code>
            </div>
          )}
          {isFileChange && approval.diffLineCount !== null && (
            <p className={cn(typography.caption, 'text-content-secondary')}>
              {t('codex.fileChange', '{{count}} diff lines', { count: approval.diffLineCount })}
            </p>
          )}
          {approval.networkHost && (
            <p className={cn(typography.caption, 'mt-1.5 text-semantic-warning')}>
              {t('codex.networkHost', 'Network: {{host}}', { host: sanitizeControlChars(approval.networkHost) })}
            </p>
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
            {t('codex.autoDecline', 'Auto-decline in {{seconds}}s', { seconds: remaining })}
          </span>
          <div className="flex gap-2">
            <button
              type="button"
              disabled={submitting}
              onClick={() => void handleSubmit('cancel')}
              className={cn(
                'rounded-lg px-4 py-1.5 text-xs',
                typography.weight.medium,
                motion.colors,
                'bg-content-inverse/10 text-content-secondary hover:bg-content-inverse/20',
                'disabled:cursor-not-allowed disabled:opacity-50',
              )}
            >
              {t('codex.cancel', 'Cancel')}
            </button>
            <button
              ref={declineButtonRef}
              type="button"
              disabled={submitting}
              onClick={() => void handleSubmit('decline')}
              className={cn(
                'rounded-lg px-4 py-1.5 text-xs',
                typography.weight.medium,
                motion.colors,
                'bg-semantic-error/15 text-semantic-error hover:bg-semantic-error/25',
                'disabled:cursor-not-allowed disabled:opacity-50',
              )}
            >
              {t('codex.decline', 'Decline')}
            </button>
            <button
              type="button"
              disabled={submitting}
              onClick={() => void handleSubmit('accept')}
              className={cn(
                'rounded-lg px-4 py-1.5 text-xs',
                typography.weight.medium,
                motion.colors,
                'bg-semantic-success/15 text-semantic-success hover:bg-semantic-success/25',
                'disabled:cursor-not-allowed disabled:opacity-50',
              )}
            >
              {t('codex.accept', 'Accept')}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
