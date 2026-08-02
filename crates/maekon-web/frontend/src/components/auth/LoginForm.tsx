/**
 * The single sign-in form (#9603 WD-02.1).
 *
 * Consumed by BOTH `setting-tabs/GeneralTab.tsx::AccountSection` (the settings
 * row) and `pages/auth/LoginPage.tsx` (the demo's first-class sign-in screen).
 * Forking it would fork four things that must not diverge: the `login` IPC
 * argument names, the `auth.failed` → localized-sentence mapping in
 * `loginFormIpcErrorKey`, the `settings.account.login.*` key set that
 * `i18n/__tests__/tauriIpcErrors.test.ts` pins across five locales, and the
 * label/`htmlFor` wiring that makes the three fields reachable by name.
 *
 * i18n note: the keys stay under `settings.account.login.*` even though the
 * form now renders outside Settings. They are the keys `TAURI_IPC_ERROR_KEYS`
 * and `LOGIN_FORM_IPC_ERROR_KEYS` resolve to, so renaming them would move the
 * error catalog for no user-visible gain. The *screen's* own chrome (title,
 * "continue without signing in", …) lives under the top-level `login.*`
 * namespace instead.
 */

import { useCallback, useId, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { addToast } from '../../hooks/useToast'
import { describeIpcError, loginFormIpcErrorKey } from '../../i18n/tauriIpcErrors'
import { form, typography } from '../../styles/tokens'
import { Button, Input } from '../ui'
import { type AuthStatus, requestLogin } from './authIpc'

export interface LoginFormProps {
  /**
   * Called with the status `login` returned, once the password has been
   * cleared. Implementations re-read `auth_status` so what is displayed is
   * server truth rather than what the command implied, and (on the login
   * screen) navigate onward.
   */
  onAuthenticated: (next: AuthStatus) => void | Promise<void>
  /**
   * Heading element for the form's own title. Settings renders it inside a
   * card that already owns the `h2`, the login screen owns the page heading —
   * so the caller picks the level rather than the form guessing.
   */
  headingLevel?: 'h2' | 'h3'
  /** Extra classes for the submit button (the screen stretches it full-width). */
  submitClassName?: string
}

export function LoginForm({ onAuthenticated, headingLevel = 'h3', submitClassName }: LoginFormProps) {
  const { t, i18n } = useTranslation()
  const [identifier, setIdentifier] = useState('')
  const [password, setPassword] = useState('')
  const [organizationId, setOrganizationId] = useState('')
  const [signingIn, setSigningIn] = useState(false)
  // Both surfaces can be mounted in the same document (Settings opened over the
  // login screen), so the field ids have to be instance-scoped or the second
  // form's labels would point at the first form's inputs.
  const fieldId = useId()
  const Heading = headingLevel

  const describeLoginError = useCallback(
    (e: unknown) => describeIpcError(e, (key) => t(key), i18n.language, loginFormIpcErrorKey),
    [i18n.language, t],
  )

  const handleLogin = useCallback(
    async (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault()
      if (signingIn) return
      setSigningIn(true)
      try {
        const next = await requestLogin({ identifier, password, organizationId })
        setPassword('')
        addToast('success', t('settings.account.login.success'))
        await onAuthenticated(next)
      } catch (e) {
        addToast('error', t('settings.account.login.error', { error: describeLoginError(e) }))
      } finally {
        setSigningIn(false)
      }
    },
    [describeLoginError, identifier, onAuthenticated, organizationId, password, signingIn, t],
  )

  return (
    <form className="space-y-3" onSubmit={handleLogin} aria-labelledby={`${fieldId}-heading`}>
      <div>
        <Heading id={`${fieldId}-heading`} className={`${typography.label} text-content-strong`}>
          {t('settings.account.login.title')}
        </Heading>
        <p className={form.helper}>{t('settings.account.login.description')}</p>
      </div>
      <div>
        <label htmlFor={`${fieldId}-identifier`} className={form.label}>
          {t('settings.account.login.identifierLabel')}
        </label>
        <Input
          id={`${fieldId}-identifier`}
          required
          type="text"
          autoComplete="username"
          value={identifier}
          onChange={(e) => setIdentifier(e.target.value)}
        />
      </div>
      <div>
        <label htmlFor={`${fieldId}-password`} className={form.label}>
          {t('settings.account.login.passwordLabel')}
        </label>
        <Input
          id={`${fieldId}-password`}
          required
          type="password"
          autoComplete="current-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </div>
      <div>
        <label htmlFor={`${fieldId}-organization-id`} className={form.label}>
          {t('settings.account.login.organizationIdLabel')}
        </label>
        <Input
          id={`${fieldId}-organization-id`}
          required
          type="text"
          autoComplete="organization"
          value={organizationId}
          onChange={(e) => setOrganizationId(e.target.value)}
        />
      </div>
      <Button type="submit" variant="primary" size="sm" isLoading={signingIn} className={submitClassName}>
        {t('settings.account.login.submit')}
      </Button>
    </form>
  )
}
