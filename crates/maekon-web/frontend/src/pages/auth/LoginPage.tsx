/**
 * `/login` — the demo's first-class sign-in screen (#9603 WD-02.1).
 *
 * Before this page the only way to sign a device in was Settings → General →
 * Account, three clicks deep behind a tab tree. Demo beat 1 (0:00–0:40) opens
 * on a sign-in screen, so the flow needs a destination that is one navigation
 * away and shows nothing but the credentials.
 *
 * The form itself is `components/auth/LoginForm` — the same component the
 * Settings section renders. This page contributes only the four-state shell
 * around it.
 *
 * ## Four states, all honest
 *
 * | `auth_status`                         | rendered                                   |
 * |---------------------------------------|--------------------------------------------|
 * | not yet read                          | spinner                                    |
 * | `server_feature: false`               | "not in this build" notice + way onward    |
 * | `server_feature`, `authenticated`      | who you are + continue                     |
 * | `server_feature`, not authenticated    | the sign-in form + continue-without        |
 *
 * A build without `--features server` (the default, and the OSS positioning:
 * connected mode is preview-only and intentionally gated) answers
 * `server_feature: false`, and so does the standalone browser dashboard where
 * the IPC bridge is absent entirely. Both land on the notice rather than a form
 * that could never submit — the route stays reachable and truthful instead of
 * 404-ing or offering dead controls.
 *
 * ## Signing in is never mandatory
 *
 * Every state offers a link onward to the dashboard. Maekon is a local-first
 * product: the whole feature set works without an account, and connected mode
 * only adds to it. This page must never become a wall.
 */

import { LogIn } from 'lucide-react'
import { useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { LoginForm } from '../../components/auth/LoginForm'
import { useAuthStatus } from '../../components/auth/useAuthStatus'
import { Alert, Button, Card, CardContent, Spinner } from '../../components/ui'
import { typography } from '../../styles/tokens'

/**
 * Where a **successful sign-in** lands (#9611 WD-02.3).
 *
 * `/home` is the context home: the operator's own mail threads, messenger
 * history and projects. It only has content once connected mode is compiled in
 * and signed in, which is exactly the state reaching this constant implies.
 *
 * The two "continue anyway" exits deliberately do NOT come here — see
 * [`WITHOUT_SIGN_IN_DESTINATION`].
 */
const POST_LOGIN_DESTINATION = '/home'

/**
 * Where the two "continue without signing in" exits land.
 *
 * Still `/` — the local-first activity dashboard, which works with no account.
 * Sending an unauthenticated user to `/home` would land them on a page whose
 * only honest render is an error, which is the same defect class as a form that
 * could never submit.
 */
const WITHOUT_SIGN_IN_DESTINATION = '/'

export default function LoginPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { status } = useAuthStatus()

  /**
   * The single navigation out of this screen — used by "continue without
   * signing in", "continue to dashboard", and a successful sign-in alike.
   *
   * Deliberately no post-login `auth_status` re-read here, unlike
   * `AccountSection`. That re-read exists because Settings *keeps rendering*
   * the session summary and must show server truth rather than what `login`
   * implied; this page unmounts on the very next line, so a refreshed status
   * would be written into state nobody reads. The destination mounts its own
   * `useAuthStatus` when it needs one.
   */
  const goToHome = useCallback(() => {
    navigate(POST_LOGIN_DESTINATION)
  }, [navigate])

  /**
   * The unauthenticated exit. Separate from [`goToHome`] because the context
   * home has nothing to show a user who is not signed in — one callback for
   * both would send them to a page whose only honest render is an error.
   */
  const goToDashboard = useCallback(() => {
    navigate(WITHOUT_SIGN_IN_DESTINATION)
  }, [navigate])

  return (
    <div className="flex min-h-full items-center justify-center p-6">
      <Card className="w-full max-w-md">
        <CardContent className="space-y-4 p-6">
          <div className="space-y-2 text-center">
            <LogIn className="mx-auto h-8 w-8 text-brand-signal" aria-hidden="true" />
            <h1 className={typography.h2}>{t('login.pageTitle')}</h1>
            <p className="text-content-secondary text-sm">{t('login.pageSubtitle')}</p>
          </div>

          {status === null && (
            <div className="flex justify-center py-6">
              <Spinner />
            </div>
          )}

          {status !== null && !status.server_feature && (
            <Alert variant="info" title={t('login.unavailableTitle')}>
              {t('settings.account.login.notAvailable')}
            </Alert>
          )}

          {status?.server_feature && status.authenticated && (
            <Alert variant="success" title={t('login.signedInTitle')}>
              {t('settings.account.login.signedInAs', {
                identifier: status.identifier ?? '',
                organizationId: status.organization_id ?? '',
              })}
            </Alert>
          )}

          {status?.server_feature && !status.authenticated && (
            <LoginForm onAuthenticated={goToHome} headingLevel="h2" submitClassName="w-full" />
          )}

          {status !== null && (
            <div className="space-y-2 border-DEFAULT border-t pt-4">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="w-full"
                onClick={status.authenticated ? goToHome : goToDashboard}
              >
                {status.authenticated ? t('login.continueToHome') : t('login.continueWithoutSignIn')}
              </Button>
              <p className="text-center text-content-secondary text-xs">{t('login.localFirstNote')}</p>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
