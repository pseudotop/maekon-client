/**
 * AutostartOnboardingPromptHost — ShowPromptCoordinator.
 * Fetches autostart config via IPC, listens for `autostart:eligible-for-prompt`
 * Tauri event, evaluates eligibility, and shows the prompt once per app session.
 *
 * Module-level singleton prevents re-show on re-mount.
 * Vite HMR resets this in dev — accepted as known dev quirk (per spec §5.5).
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { type AutostartConfig, AutostartOnboardingPrompt } from './AutostartOnboardingPrompt'

async function invokeDesktop<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

async function listenDesktop<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event')
  const unlisten = await listen<T>(event, (e) => handler(e.payload))
  return unlisten
}

// Module-level singleton: prevents re-show on re-mount within a single app session.
let hasShownThisSession = false

function shouldShowPrompt(cfg: AutostartConfig): boolean {
  if (cfg.prompt_state.kind === 'dismissed') return false
  if (cfg.prompt_state.kind === 'pending') return cfg.productive_session_count >= 1
  if (cfg.prompt_state.kind === 'snoozed') {
    const threshold = cfg.prompt_state.remind_after_session_count ?? Number.POSITIVE_INFINITY
    return cfg.productive_session_count >= threshold
  }
  return false
}

export function AutostartOnboardingPromptHost() {
  const [shouldShow, setShouldShow] = useState(false)
  const [config, setConfig] = useState<AutostartConfig | null>(null)
  const timerRef = useRef<number | null>(null)

  const evaluate = useCallback(async () => {
    if (hasShownThisSession) return
    try {
      const cfg = await invokeDesktop<AutostartConfig>('get_autostart_config')
      setConfig(cfg)
      if (shouldShowPrompt(cfg)) {
        if (timerRef.current === null) {
          timerRef.current = window.setTimeout(() => {
            if (!hasShownThisSession) {
              setShouldShow(true)
              hasShownThisSession = true
            }
            timerRef.current = null
          }, 500)
        }
      }
    } catch (e) {
      console.debug('[autostart-prompt] eligibility check failed', e)
    }
  }, [])

  useEffect(() => {
    void evaluate()
    let unlisten: (() => void) | null = null
    // Listener registration race (spec §5.5 accepted): if the Tauri event fires
    // between evaluate() resolving and listenDesktop() resolving (~1-2ms window),
    // it is missed. The initial evaluate() above serves as the primary trigger
    // for the first eligible session; subsequent sessions use the listener.
    void listenDesktop('autostart:eligible-for-prompt', () => void evaluate()).then((fn) => {
      unlisten = fn
    })
    return () => {
      unlisten?.()
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current)
        timerRef.current = null
      }
    }
  }, [evaluate])

  if (!shouldShow || !config) return null
  return <AutostartOnboardingPrompt config={config} onClose={() => setShouldShow(false)} />
}
