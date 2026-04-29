/**
 * AutostartOnboardingPrompt — modal dialog asking user to enable autostart.
 * Uses the shared Dialog + Button UI components for style consistency.
 */
import { useTranslation } from 'react-i18next'
import { Button } from './ui/Button'
import { Dialog, DialogBody, DialogContent, DialogFooter, DialogTitle } from './ui/Dialog'

async function invokeDesktop<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

export interface AutostartConfig {
  prompt_state: { kind: string; remind_after_session_count?: number }
  productive_session_count: number
  last_session_id: string | null
}

interface Props {
  config: AutostartConfig
  onClose: () => void
}

export function AutostartOnboardingPrompt({ config, onClose }: Props) {
  const { t } = useTranslation()

  async function handleEnable() {
    try {
      await invokeDesktop('enable_autostart')
      await invokeDesktop('mark_autostart_prompt_state', { newState: { kind: 'dismissed' } })
    } finally {
      onClose()
    }
  }

  async function handleNotNow() {
    try {
      await invokeDesktop('mark_autostart_prompt_state', {
        newState: {
          kind: 'snoozed',
          remind_after_session_count: config.productive_session_count + 5,
        },
      })
    } finally {
      onClose()
    }
  }

  async function handleDismiss() {
    try {
      await invokeDesktop('mark_autostart_prompt_state', { newState: { kind: 'dismissed' } })
    } finally {
      onClose()
    }
  }

  return (
    // Dialog handles Escape + backdrop click → onClose maps to handleNotNow via wrapper
    <Dialog open onClose={() => void handleNotNow()}>
      <DialogContent size="sm">
        <DialogTitle>{t('onboarding.autostart.title')}</DialogTitle>
        <DialogBody>{t('onboarding.autostart.body')}</DialogBody>
        <DialogFooter className="flex-wrap gap-2">
          <Button variant="ghost" size="sm" onClick={() => void handleDismiss()} type="button">
            {t('onboarding.autostart.dismiss_button')}
          </Button>
          <Button variant="secondary" size="sm" onClick={() => void handleNotNow()} type="button">
            {t('onboarding.autostart.not_now_button')}
          </Button>
          <Button variant="primary" size="sm" onClick={() => void handleEnable()} type="button">
            {t('onboarding.autostart.enable_button')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
