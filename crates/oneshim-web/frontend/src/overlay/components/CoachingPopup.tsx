import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { motion, typography } from '../../styles/tokens'
import { useAutoDismiss } from '../hooks/useAutoDismiss'
import type { CoachingPayload, DismissAction } from '../types'

async function tauriInvoke(cmd: string, args?: Record<string, unknown>): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core')
  await invoke(cmd, args)
}

interface CoachingPopupProps {
  message: CoachingPayload
  autoDismissSecs: number
}

export default function CoachingPopup({ message, autoDismissSecs }: CoachingPopupProps) {
  const { t } = useTranslation()
  const [text, setText] = useState(message.text)
  const [transitioning, setTransitioning] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const prevTextRef = useRef(message.text)

  // Detect text upgrade (LLM personalization)
  useEffect(() => {
    if (message.text !== prevTextRef.current) {
      setTransitioning(true)
      const timer = setTimeout(() => {
        setText(message.text)
        setTransitioning(false)
      }, 300) // fade duration
      prevTextRef.current = message.text
      return () => clearTimeout(timer)
    }
    setText(message.text)
  }, [message.text])

  const dismiss = useCallback(
    async (action: DismissAction) => {
      await tauriInvoke('dismiss_coaching_message', {
        messageId: message.message_id,
        action,
        profile: message.profile,
      })
    },
    [message.message_id, message.profile],
  )

  // The auto-dismiss timer is intentionally NOT reset when an LLM text
  // upgrade arrives.  Previously the timer restarted on every text change,
  // which extended popup visibility unpredictably — users expected the
  // original 15-second window and were surprised when the popup lingered.
  // The text still fades in (via the transition effect above), but the
  // countdown continues from where it was.
  useAutoDismiss(
    true,
    autoDismissSecs,
    () => void dismiss('timeout').catch((e) => console.warn('dismiss_coaching_message(timeout) failed:', e)),
  )

  const [feedbackSent, setFeedbackSent] = useState<'positive' | 'negative' | null>(null)

  const feedback = useCallback(
    async (positive: boolean) => {
      await tauriInvoke('submit_coaching_feedback', {
        messageId: message.message_id,
        positive,
      })
      setFeedbackSent(positive ? 'positive' : 'negative')
    },
    [message.message_id],
  )

  // Dismiss sends the action to backend. Implicit feedback (regime/app change
  // within 5 min) is evaluated separately by FeedbackTracker — no need to send
  // explicit feedback on dismiss. Only thumbs-up/down count as explicit signals.
  const handleDismiss = useCallback(
    async (action: DismissAction) => {
      try {
        setError(null)
        await dismiss(action)
      } catch (e) {
        console.warn(`dismiss_coaching_message(${action}) failed:`, e)
        setError(t('coaching.dismissError', 'Could not update the coaching message.'))
      }
    },
    [dismiss, t],
  )

  const handleFeedback = useCallback(
    async (positive: boolean) => {
      try {
        setError(null)
        await feedback(positive)
      } catch (e) {
        console.warn(`submit_coaching_feedback(${positive ? 'positive' : 'negative'}) failed:`, e)
        setError(t('coaching.feedbackError', 'Could not save feedback.'))
      }
    },
    [feedback, t],
  )

  // Note: No per-element mouseenter/mouseleave cursor passthrough management.
  // The overlay is click-through by default. The user presses Cmd+Shift+O to
  // make it interactive. After dismissal, the Rust backend returns it to
  // click-through mode automatically.

  return (
    <div className="fixed top-4 right-4 z-overlay">
      <div className="w-80 max-w-[calc(100vw-2rem)] rounded-xl border border-content-inverse/10 bg-surface-sunken/90 p-4 shadow-2xl backdrop-blur-md">
        {/* Message text with transition */}
        <p
          className={`mb-3 text-content text-sm leading-relaxed ${motion.opacity}${
            transitioning ? 'opacity-0' : 'opacity-100'
          }`}
        >
          {text}
        </p>
        {error && <p className="mb-3 text-semantic-error text-xs">{error}</p>}

        {/* Why? expandable explanation */}
        {message.explanation && (
          <details className="mt-2 mb-3">
            <summary className="cursor-pointer text-content-tertiary text-xs hover:text-content-secondary">
              {t('coaching.whyTitle', 'Why this suggestion?')}
            </summary>
            <p className="mt-1 text-content-secondary text-xs leading-relaxed">{message.explanation}</p>
          </details>
        )}

        {/* Actions row */}
        <div className="flex items-center justify-between">
          <div className="flex gap-2">
            <button
              type="button"
              onClick={() => void handleDismiss('ok')}
              aria-label={t('coaching.dismissLabel', 'Dismiss coaching message')}
              className={`rounded-md bg-content-inverse/10 px-3 py-1 text-xs ${typography.weight.medium} text-content-secondary ${motion.colors} hover:bg-content-inverse/20`}
            >
              {t('coaching.ok', 'OK')}
            </button>
            <button
              type="button"
              onClick={() => void handleDismiss('later')}
              aria-label={t('coaching.laterLabel', 'Remind me later')}
              className={`rounded-md bg-content-inverse/5 px-3 py-1 text-xs ${typography.weight.medium} text-content-tertiary ${motion.colors} hover:bg-content-inverse/10`}
            >
              {t('coaching.later', 'Later')}
            </button>
          </div>

          {/* Thumbs feedback — subtle by default, shows confirmation after submit */}
          <div className="flex items-center gap-1">
            {feedbackSent ? (
              <span
                className={`text-[10px] ${feedbackSent === 'positive' ? 'text-semantic-success' : 'text-semantic-error'} ${motion.opacity}`}
              >
                {feedbackSent === 'positive'
                  ? t('coaching.thanksFeedback', 'Thanks! Learning...')
                  : t('coaching.adjustingFeedback', 'Got it, adjusting...')}
              </span>
            ) : (
              <>
                <button
                  type="button"
                  onClick={() => void handleFeedback(true)}
                  className={`rounded p-1.5 text-content-muted opacity-30 ${motion.opacity} hover:text-semantic-success hover:opacity-100`.trim()}
                  aria-label={t('coaching.helpfulLabel', 'Helpful')}
                >
                  <ThumbsUpIcon />
                </button>
                <button
                  type="button"
                  onClick={() => void handleFeedback(false)}
                  className={`rounded p-1.5 text-content-muted opacity-30 ${motion.opacity} hover:text-semantic-error hover:opacity-100`.trim()}
                  aria-label={t('coaching.notHelpfulLabel', 'Not helpful')}
                >
                  <ThumbsDownIcon />
                </button>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}

function ThumbsUpIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <title>Thumbs up</title>
      <path d="M7 10v12" />
      <path d="M15 5.88 14 10h5.83a2 2 0 0 1 1.92 2.56l-2.33 8A2 2 0 0 1 17.5 22H4a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2h2.76a2 2 0 0 0 1.79-1.11L12 2h0a3.13 3.13 0 0 1 3 3.88Z" />
    </svg>
  )
}

function ThumbsDownIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <title>Thumbs down</title>
      <path d="M17 14V2" />
      <path d="M9 18.12 10 14H4.17a2 2 0 0 1-1.92-2.56l2.33-8A2 2 0 0 1 6.5 2H20a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-2.76a2 2 0 0 0-1.79 1.11L12 22h0a3.13 3.13 0 0 1-3-3.88Z" />
    </svg>
  )
}
