import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface SessionTurnControlProps {
  /** The active session whose in-flight turn this control steers/interrupts. */
  sessionId: string
}

/**
 * Thin mid-turn control (E21 #5017): an Interrupt button + a steer input/Send.
 * This is the LIVE CONSUMER of the new `ConversationSession::interrupt`/`steer`
 * trait methods (Tauri `interrupt_session_turn` / `steer_session_turn`
 * commands) — without it those trait methods would have no frontend caller
 * (dead-writer ban). Rendered only when an active session id is known.
 *
 * Uses dynamic `await import('@tauri-apps/api/core')` for graceful degradation
 * outside Tauri (matches the overlay IPC convention).
 */
export function SessionTurnControl({ sessionId }: SessionTurnControlProps) {
  const { t } = useTranslation()
  const [steerText, setSteerText] = useState('')
  const [busy, setBusy] = useState(false)

  const interrupt = useCallback(async () => {
    if (busy) return
    setBusy(true)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('interrupt_session_turn', { sessionId })
    } catch (e) {
      console.warn('interrupt_session_turn failed:', e)
    } finally {
      setBusy(false)
    }
  }, [busy, sessionId])

  const steer = useCallback(async () => {
    const message = steerText.trim()
    if (busy || message.length === 0) return
    setBusy(true)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('steer_session_turn', { sessionId, message })
      setSteerText('')
    } catch (e) {
      console.warn('steer_session_turn failed:', e)
    } finally {
      setBusy(false)
    }
  }, [busy, sessionId, steerText])

  return (
    <fieldset className="flex items-center gap-2 border-0 p-0" aria-label={t('codex.turnControl', 'Turn control')}>
      <button
        type="button"
        disabled={busy}
        onClick={() => void interrupt()}
        className={cn(
          'rounded-lg px-3 py-1.5 text-xs',
          typography.weight.medium,
          motion.colors,
          'bg-semantic-error/15 text-semantic-error hover:bg-semantic-error/25',
          'disabled:cursor-not-allowed disabled:opacity-50',
        )}
      >
        {t('codex.interrupt', 'Interrupt')}
      </button>
      <input
        type="text"
        value={steerText}
        disabled={busy}
        onChange={(e) => setSteerText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void steer()
        }}
        placeholder={t('codex.steerPlaceholder', 'Steer the turn…')}
        className={cn(
          'flex-1 rounded-lg border border-content-inverse/10 bg-content-inverse/5 px-3 py-1.5 text-content text-xs',
          'placeholder:text-content-tertiary disabled:opacity-50',
        )}
      />
      <button
        type="button"
        disabled={busy || steerText.trim().length === 0}
        onClick={() => void steer()}
        className={cn(
          'rounded-lg px-3 py-1.5 text-xs',
          typography.weight.medium,
          motion.colors,
          'bg-brand/15 text-brand hover:bg-brand/25',
          'disabled:cursor-not-allowed disabled:opacity-50',
        )}
      >
        {t('codex.steerSend', 'Send')}
      </button>
    </fieldset>
  )
}
