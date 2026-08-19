/**
 * `/home` — the operator's context home (#9611 WD-02.3).
 *
 * Demo beat 1 ends here: sign in, and land on your own mail threads, messenger
 * history and projects. Everything on this page comes from the single #9625
 * typed IPC call and nothing else — no REST, no second source, no client-side
 * merge. That is deliberate: the server resolves *whose* home this is from the
 * JWT, and a second data path would be a second place to get that wrong.
 *
 * ## Six states, kept apart
 *
 * `useContextHome` distinguishes loading / ready / stale / unavailable / reauth
 * / denied, and each section additionally reports ready / empty / unavailable /
 * unknown. The failure this page exists to prevent is collapsing any of those
 * into "nothing here" — three of them are the user's to act on and three are
 * not, and an empty panel tells them nothing about which.
 *
 * ## Session expiry leaves; permission denial does not
 *
 * `reauth` redirects to `/login`, because re-signing-in is the fix. `denied`
 * stays put and says so: the user is signed in and simply may not read this,
 * and sending them to a login screen would be a loop with no exit.
 *
 * ## Bounded by construction
 *
 * The server already caps the payload (20 + 20 + 10 rows, 12 participants per
 * thread). This page caps again at legibility limits and states the remainder
 * as a count. At 50-person scale the participant strip, not the row count, is
 * what makes a thread unreadable.
 */

import { AlertTriangle, ExternalLink, Inbox, MessageSquare, RefreshCw, ShieldOff } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import {
  CONSOLE_HANDOFF_ERROR_CODES,
  ConsoleHandoffBridgeUnavailableError,
  type ConsoleHandoffReceipt,
  openConsoleAssignmentBoard,
} from '../../api/consoleHandoff'
import type { ContextHomeProject, ContextHomeSnapshot, ContextHomeThread } from '../../api/contextHome'
import { isIpcError } from '../../api/desktop'
import { markSyntheticSession } from '../../components/shell/syntheticSessionSignal'
import { Alert, Button, Card, CardContent, Spinner } from '../../components/ui'
import { iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import {
  formatAsOf,
  hiddenRowCount,
  homeCompleteness,
  MAX_RENDERED_PARTICIPANTS,
  MAX_RENDERED_PROJECTS,
  MAX_RENDERED_THREADS,
  type SectionRenderState,
  sectionRenderState,
} from './homeSections'
import { useContextHome } from './useContextHome'

/** Where an expired session is sent. Re-login is the fix, so it is offered. */
const LOGIN_ROUTE = '/login'

export default function ContextHomePage() {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const { view, refresh, refreshing } = useContextHome()

  const snapshot = view.kind === 'ready' ? view.snapshot : null

  // Latch the demo label as soon as the server states it. Done here rather than
  // inside the hook so the hook stays a pure data concern and the shell-level
  // side effect is visible on the page that owns the claim.
  useEffect(() => {
    if (!snapshot) return
    markSyntheticSession({
      synthetic: snapshot.synthetic || snapshot.provenance.synthetic_only,
      seedNamespaces: snapshot.provenance.seed_namespaces,
    })
  }, [snapshot])

  // An expired session is the one state that belongs somewhere else.
  useEffect(() => {
    if (view.kind === 'reauth') navigate(LOGIN_ROUTE, { replace: true })
  }, [view.kind, navigate])

  const announcement = useHomeAnnouncement(view.kind, snapshot, refreshing)

  return (
    <div className="mx-auto w-full max-w-5xl p-4 sm:p-6" data-testid="context-home">
      {/*
        One live region for the whole page. Screen-reader users get told when the
        home goes stale, partial, or unavailable — transitions that are otherwise
        purely visual and would pass silently.
      */}
      <output className="sr-only" aria-live="polite" aria-atomic="true" data-testid="context-home-announcement">
        {announcement}
      </output>

      <HomeHeader view={view} snapshot={snapshot} refreshing={refreshing} onRefresh={refresh} locale={i18n.language} />

      {snapshot?.synthetic && snapshot.provenance.synthetic_only && snapshot.provenance.seed_namespaces.length > 0 && (
        <ConsoleHandoffAction onSessionExpired={() => navigate(LOGIN_ROUTE, { replace: true })} />
      )}

      {view.kind === 'loading' && <HomeLoading />}
      {view.kind === 'unavailable' && <HomeUnavailable onRetry={refresh} />}
      {view.kind === 'denied' && <HomeDenied />}
      {view.kind === 'bridgeAbsent' && <HomeBridgeAbsent />}
      {view.kind === 'malformed' && <HomeMalformed onRetry={refresh} />}
      {view.kind === 'reauth' && (
        // Rendered for the frame before the redirect lands, and for any host
        // that suppresses navigation. Never a dead end.
        <Alert variant="warning" data-testid="context-home-reauth">
          {t('contextHome.reauth.body', 'Your session has expired. Sign in again to see your context.')}
        </Alert>
      )}

      {snapshot && <HomeSections snapshot={snapshot} />}
    </div>
  )
}

type ConsoleHandoffView =
  | { kind: 'idle' }
  | { kind: 'opening' }
  | { kind: 'opened'; receipt: ConsoleHandoffReceipt }
  | { kind: 'error'; code: string }

function ConsoleHandoffAction({ onSessionExpired }: { onSessionExpired: () => void }) {
  const { t } = useTranslation()
  const inFlight = useRef(false)
  const [view, setView] = useState<ConsoleHandoffView>({ kind: 'idle' })

  const open = async () => {
    if (inFlight.current) return
    inFlight.current = true
    setView({ kind: 'opening' })
    try {
      const receipt = await openConsoleAssignmentBoard()
      setView({ kind: 'opened', receipt })
    } catch (error) {
      if (isIpcError(error) && error.code === CONSOLE_HANDOFF_ERROR_CODES.sessionExpired) {
        onSessionExpired()
        return
      }
      const code =
        error instanceof ConsoleHandoffBridgeUnavailableError
          ? 'bridge.absent'
          : isIpcError(error)
            ? error.code
            : 'validation.invalid_field'
      setView({ kind: 'error', code })
    } finally {
      inFlight.current = false
    }
  }

  return (
    <Card className="mb-6" data-testid="console-handoff-card">
      <CardContent className="flex flex-wrap items-center justify-between gap-3 p-4">
        <div>
          <p className={cn(typography.body, typography.weight.medium)}>
            {t('contextHome.consoleHandoff.title', 'Continue in Console')}
          </p>
          <p className={cn(typography.caption, 'text-content-secondary')}>
            {t(
              'contextHome.consoleHandoff.body',
              'Open the assignment board with this synthetic run and source snapshot verified by your current session.',
            )}
          </p>
        </div>
        <Button
          variant="primary"
          size="sm"
          disabled={view.kind === 'opening'}
          onClick={open}
          data-testid="console-handoff-open"
        >
          <ExternalLink className="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
          {view.kind === 'opening'
            ? t('contextHome.consoleHandoff.opening', 'Opening…')
            : t('contextHome.consoleHandoff.action', 'Open assignment board')}
        </Button>
        {view.kind === 'opened' && (
          <Alert variant="success" className="w-full" data-testid="console-handoff-opened">
            {t('contextHome.consoleHandoff.opened', {
              defaultValue: 'Console opened for run {{run}}.',
              run: view.receipt.run_id,
            })}
          </Alert>
        )}
        {view.kind === 'error' && (
          <Alert variant="warning" className="w-full" data-testid="console-handoff-error" data-error-code={view.code}>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span>{consoleHandoffErrorText(t, view.code)}</span>
              {isRetryableConsoleHandoffCode(view.code) && (
                <Button variant="secondary" size="sm" onClick={open} data-testid="console-handoff-retry">
                  {t('contextHome.retry', 'Try again')}
                </Button>
              )}
            </div>
          </Alert>
        )}
      </CardContent>
    </Card>
  )
}

function isRetryableConsoleHandoffCode(code: string): boolean {
  return new Set<string>([
    CONSOLE_HANDOFF_ERROR_CODES.unavailable,
    CONSOLE_HANDOFF_ERROR_CODES.timeout,
    CONSOLE_HANDOFF_ERROR_CODES.rateLimit,
    CONSOLE_HANDOFF_ERROR_CODES.launchFailed,
    CONSOLE_HANDOFF_ERROR_CODES.noHandler,
  ]).has(code)
}

function consoleHandoffErrorText(t: ReturnType<typeof useTranslation>['t'], code: string): string {
  if (code === CONSOLE_HANDOFF_ERROR_CODES.permissionDenied) {
    return t('contextHome.consoleHandoff.errors.denied', 'This account cannot continue this synthetic run in Console.')
  }
  if (code === CONSOLE_HANDOFF_ERROR_CODES.configMissing || code === CONSOLE_HANDOFF_ERROR_CODES.configInvalid) {
    return t('contextHome.consoleHandoff.errors.config', 'The Console destination is not configured correctly.')
  }
  if (code.startsWith('handoff.')) {
    return t(
      'contextHome.consoleHandoff.errors.launch',
      'Console could not be opened. Check the browser handler and retry.',
    )
  }
  if (code === 'bridge.absent') {
    return t('contextHome.consoleHandoff.errors.bridge', 'This transition is available only in the Maekon desktop app.')
  }
  return t('contextHome.consoleHandoff.errors.unavailable', 'The Console transition is temporarily unavailable.')
}

/* -------------------------------------------------------------------------- */
/* Header                                                                      */
/* -------------------------------------------------------------------------- */

function HomeHeader({
  view,
  snapshot,
  refreshing,
  onRefresh,
  locale,
}: {
  view: ReturnType<typeof useContextHome>['view']
  snapshot: ContextHomeSnapshot | null
  refreshing: boolean
  onRefresh: () => void
  locale: string
}) {
  const { t } = useTranslation()
  const stale = view.kind === 'ready' && view.stale

  return (
    <header className="mb-6 flex flex-wrap items-start justify-between gap-3">
      <div className="min-w-0">
        <h1 className={cn(typography.h2, 'text-content')}>{t('contextHome.title', 'Your context')}</h1>
        {snapshot && (
          <p className={cn(typography.caption, 'mt-1 text-content-secondary')} data-testid="context-home-actor">
            {t('contextHome.actorLine', {
              defaultValue: '{{actor}} · {{org}}',
              actor: snapshot.actor.actor_id,
              org: snapshot.actor.organization_id,
            })}
            {' · '}
            <span data-testid="context-home-as-of">
              {formatAsOf(snapshot.as_of, snapshot.timezone, locale)} ({snapshot.timezone})
            </span>
          </p>
        )}
      </div>

      <div className="flex items-center gap-2">
        {stale && (
          // Not colour alone: an icon and the word "stale" carry the meaning.
          <span
            data-testid="context-home-stale"
            className={cn(
              'flex items-center gap-1 rounded border border-semantic-warning/60 px-2 py-0.5 text-semantic-warning',
              typography.micro,
              typography.weight.medium,
            )}
          >
            <AlertTriangle className={iconSize.xs} aria-hidden="true" />
            {t('contextHome.stale.badge', 'Showing last known data')}
          </span>
        )}
        <Button
          variant="secondary"
          size="sm"
          onClick={onRefresh}
          disabled={refreshing}
          data-testid="context-home-refresh"
        >
          <RefreshCw className={cn('mr-1.5 h-3.5 w-3.5', refreshing && 'animate-spin')} aria-hidden="true" />
          {t('contextHome.refresh', 'Refresh')}
        </Button>
      </div>
    </header>
  )
}

/* -------------------------------------------------------------------------- */
/* Whole-page states                                                           */
/* -------------------------------------------------------------------------- */

function HomeLoading() {
  const { t } = useTranslation()
  return (
    <div className="flex items-center gap-2 py-12 text-content-secondary" data-testid="context-home-loading">
      <Spinner />
      <span className={typography.body}>{t('contextHome.loading', 'Loading your context…')}</span>
    </div>
  )
}

function HomeUnavailable({ onRetry }: { onRetry: () => void }) {
  const { t } = useTranslation()
  return (
    <Alert variant="warning" data-testid="context-home-unavailable">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span>
          {t('contextHome.unavailable.body', 'Your context could not be loaded right now. This is usually temporary.')}
        </span>
        <Button variant="secondary" size="sm" onClick={onRetry} data-testid="context-home-unavailable-retry">
          {t('contextHome.retry', 'Try again')}
        </Button>
      </div>
    </Alert>
  )
}

function HomeDenied() {
  const { t } = useTranslation()
  // Deliberately no retry and no sign-in link. The user IS signed in; neither
  // action changes the answer, and offering them implies otherwise.
  return (
    <Alert variant="error" data-testid="context-home-denied">
      <span className="flex items-center gap-2">
        <ShieldOff className={cn(iconSize.base, 'shrink-0')} aria-hidden="true" />
        {t(
          'contextHome.denied.body',
          'This account does not have permission to view its context home. Ask an administrator for access.',
        )}
      </span>
    </Alert>
  )
}

function HomeBridgeAbsent() {
  const { t } = useTranslation()
  return (
    <Alert variant="info" data-testid="context-home-bridge-absent">
      {t(
        'contextHome.bridgeAbsent.body',
        'The context home is only available in the desktop app, not in the browser dashboard.',
      )}
    </Alert>
  )
}

function HomeMalformed({ onRetry }: { onRetry: () => void }) {
  const { t } = useTranslation()
  return (
    <Alert variant="error" data-testid="context-home-malformed">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span>
          {t(
            'contextHome.malformed.body',
            'The server sent a context response this app version cannot read. Updating the app usually resolves this.',
          )}
        </span>
        <Button variant="secondary" size="sm" onClick={onRetry} data-testid="context-home-malformed-retry">
          {t('contextHome.retry', 'Try again')}
        </Button>
      </div>
    </Alert>
  )
}

/* -------------------------------------------------------------------------- */
/* Sections                                                                    */
/* -------------------------------------------------------------------------- */

function HomeSections({ snapshot }: { snapshot: ContextHomeSnapshot }) {
  const { t } = useTranslation()

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <ThreadSection
        testId="mail"
        titleKey="contextHome.sections.mail"
        titleDefault="Mail"
        icon={Inbox}
        state={sectionRenderState(snapshot.mail)}
        threads={snapshot.mail.items}
        hidden={hiddenRowCount(snapshot.mail, MAX_RENDERED_THREADS)}
        reason={snapshot.mail.unavailable_reason ?? null}
      />
      <ThreadSection
        testId="messenger"
        titleKey="contextHome.sections.messenger"
        titleDefault="Messages"
        icon={MessageSquare}
        state={sectionRenderState(snapshot.messenger)}
        threads={snapshot.messenger.items}
        hidden={hiddenRowCount(snapshot.messenger, MAX_RENDERED_THREADS)}
        reason={snapshot.messenger.unavailable_reason ?? null}
      />
      <section
        aria-labelledby="context-home-projects-heading"
        data-testid="context-home-section-projects"
        data-section-state={sectionRenderState(snapshot.projects)}
        className="lg:col-span-2"
      >
        <Card>
          <CardContent>
            <SectionHeading
              id="context-home-projects-heading"
              label={t('contextHome.sections.projects', 'Projects')}
              state={sectionRenderState(snapshot.projects)}
            />
            <SectionBody
              state={sectionRenderState(snapshot.projects)}
              reason={snapshot.projects.unavailable_reason ?? null}
              emptyLabel={t('contextHome.empty.projects', 'You are not on any projects yet.')}
              testId="projects"
            >
              <ul className="divide-y divide-border-subtle">
                {snapshot.projects.items.slice(0, MAX_RENDERED_PROJECTS).map((project) => (
                  <ProjectRow key={project.project_id} project={project} />
                ))}
              </ul>
              <HiddenCount count={hiddenRowCount(snapshot.projects, MAX_RENDERED_PROJECTS)} />
            </SectionBody>
          </CardContent>
        </Card>
      </section>
    </div>
  )
}

function ThreadSection({
  testId,
  titleKey,
  titleDefault,
  icon: Icon,
  state,
  threads,
  hidden,
  reason,
}: {
  testId: string
  titleKey: string
  titleDefault: string
  icon: typeof Inbox
  state: SectionRenderState
  threads: ContextHomeThread[]
  hidden: number
  reason: string | null
}) {
  const { t } = useTranslation()
  const headingId = `context-home-${testId}-heading`

  return (
    <section aria-labelledby={headingId} data-testid={`context-home-section-${testId}`} data-section-state={state}>
      <Card>
        <CardContent>
          <SectionHeading id={headingId} label={t(titleKey, titleDefault)} state={state} icon={Icon} />
          <SectionBody
            state={state}
            reason={reason}
            emptyLabel={t('contextHome.empty.threads', 'No recent conversations.')}
            testId={testId}
          >
            <ul className="divide-y divide-border-subtle">
              {threads.slice(0, MAX_RENDERED_THREADS).map((thread) => (
                <ThreadRow key={thread.thread_id} thread={thread} />
              ))}
            </ul>
            <HiddenCount count={hidden} />
          </SectionBody>
        </CardContent>
      </Card>
    </section>
  )
}

function SectionHeading({
  id,
  label,
  state,
  icon: Icon,
}: {
  id: string
  label: string
  state: SectionRenderState
  icon?: typeof Inbox
}) {
  const { t } = useTranslation()
  return (
    <div className="mb-2 flex items-center gap-2">
      {Icon && <Icon className={cn(iconSize.base, 'text-content-tertiary')} aria-hidden="true" />}
      <h2 id={id} className={cn(typography.h4, 'text-content')}>
        {label}
      </h2>
      {(state === 'unavailable' || state === 'unknown') && (
        // The state is spelled out, not signalled by colour: a greyed panel and
        // an empty panel look identical in a screenshot.
        <span
          className={cn(
            'flex items-center gap-1 rounded border border-semantic-warning/60 px-1.5 py-0.5 text-semantic-warning',
            typography.weight.medium,
          )}
          data-testid={`context-home-${id}-flag`}
        >
          <AlertTriangle className={iconSize.xs} aria-hidden="true" />
          {t('contextHome.section.unavailableFlag', 'Unavailable')}
        </span>
      )}
    </div>
  )
}

function SectionBody({
  state,
  reason,
  emptyLabel,
  testId,
  children,
}: {
  state: SectionRenderState
  reason: string | null
  emptyLabel: string
  testId: string
  children: React.ReactNode
}) {
  const { t } = useTranslation()

  if (state === 'unavailable' || state === 'unknown') {
    return (
      <p
        className={cn(typography.caption, 'text-content-secondary')}
        data-testid={`context-home-${testId}-unavailable`}
      >
        {reason === 'timeout'
          ? t('contextHome.section.timeout', 'This section took too long to load and was skipped.')
          : t(
              'contextHome.section.backendUnavailable',
              'This section could not be loaded. The rest of your context is current.',
            )}
      </p>
    )
  }

  if (state === 'empty') {
    return (
      <p className={cn(typography.caption, 'text-content-secondary')} data-testid={`context-home-${testId}-empty`}>
        {emptyLabel}
      </p>
    )
  }

  return <>{children}</>
}

function HiddenCount({ count }: { count: number }) {
  const { t } = useTranslation()
  if (count <= 0) return null
  return (
    <p className={cn(typography.caption, 'mt-2 text-content-tertiary')} data-testid="context-home-hidden-count">
      {t('contextHome.moreRows', { defaultValue: '+{{count}} more', count })}
    </p>
  )
}

/* -------------------------------------------------------------------------- */
/* Rows                                                                        */
/* -------------------------------------------------------------------------- */

function ThreadRow({ thread }: { thread: ContextHomeThread }) {
  const { t } = useTranslation()
  const shown = thread.participants.slice(0, MAX_RENDERED_PARTICIPANTS)
  const overflow = Math.max(0, thread.participant_count - shown.length)

  return (
    <li className="py-2.5" data-testid="context-home-thread">
      <p className={cn(typography.label, 'truncate text-content')}>{thread.subject}</p>
      {thread.last_message_preview && (
        <p className={cn(typography.caption, 'mt-0.5 truncate text-content-secondary')}>
          {thread.last_message_preview}
        </p>
      )}
      <div className="mt-1 flex flex-wrap items-center gap-1">
        {shown.map((p) => (
          <span
            key={p.participant_id}
            data-testid="context-home-participant"
            data-participant-kind={p.kind}
            className={cn(
              'rounded px-1.5 py-0.5 text-[10px]',
              // External counterparties are marked with a border and a suffixed
              // label, not a colour: drawing an outside contact as a colleague
              // is a false statement about a real person, and colour alone does
              // not survive greyscale, high contrast, or a colour-vision
              // difference.
              p.kind === 'external_counterparty_contact'
                ? 'border border-content-tertiary/60 text-content-secondary'
                : 'bg-surface-raised text-content-secondary',
            )}
          >
            {p.kind === 'external_counterparty_contact'
              ? t('contextHome.participant.external', {
                  defaultValue: '{{name}} (external)',
                  name: p.display_label,
                })
              : p.display_label}
          </span>
        ))}
        {overflow > 0 && (
          <span className="text-[10px] text-content-tertiary" data-testid="context-home-participant-overflow">
            {t('contextHome.moreParticipants', { defaultValue: '+{{count}}', count: overflow })}
          </span>
        )}
      </div>
    </li>
  )
}

function ProjectRow({ project }: { project: ContextHomeProject }) {
  const { t } = useTranslation()
  return (
    <li className="flex flex-wrap items-baseline gap-x-2 py-2.5" data-testid="context-home-project">
      <span className={cn(typography.label, 'text-content')}>{project.name}</span>
      {project.code && <span className={cn(typography.caption, 'text-content-tertiary')}>{project.code}</span>}
      {project.my_role && (
        <span className={cn(typography.caption, 'text-content-secondary')}>
          {t('contextHome.project.role', { defaultValue: 'Role: {{role}}', role: project.my_role })}
        </span>
      )}
      {project.counterparty_label && (
        <span
          className={cn(typography.caption, 'text-content-secondary')}
          data-testid="context-home-project-counterparty"
        >
          {t('contextHome.project.counterparty', {
            defaultValue: 'Counterparty: {{name}}',
            name: project.counterparty_label,
          })}
        </span>
      )}
    </li>
  )
}

/* -------------------------------------------------------------------------- */
/* Live-region text                                                            */
/* -------------------------------------------------------------------------- */

/**
 * One sentence describing the current state, for screen readers.
 *
 * The visual distinctions on this page (a stale chip, a greyed section, an
 * amber flag) are all silent otherwise — which would make "the six states are
 * distinguishable" true only for sighted users.
 */
function useHomeAnnouncement(
  kind: ReturnType<typeof useContextHome>['view']['kind'],
  snapshot: ContextHomeSnapshot | null,
  refreshing: boolean,
): string {
  const { t } = useTranslation()

  return useMemo(() => {
    if (kind === 'loading') return t('contextHome.loading', 'Loading your context…')
    if (kind === 'unavailable') return t('contextHome.announce.unavailable', 'Your context is unavailable.')
    if (kind === 'denied') return t('contextHome.announce.denied', 'You do not have permission to view this context.')
    if (kind === 'reauth') return t('contextHome.announce.reauth', 'Your session expired. Returning to sign in.')
    if (kind === 'bridgeAbsent')
      return t('contextHome.announce.bridgeAbsent', 'The context home needs the desktop app.')
    if (kind === 'malformed') return t('contextHome.announce.malformed', 'The context response could not be read.')
    if (!snapshot) return ''
    if (refreshing) return t('contextHome.announce.refreshing', 'Refreshing your context.')

    switch (homeCompleteness(snapshot)) {
      case 'unavailable':
        return t('contextHome.announce.unavailable', 'Your context is unavailable.')
      case 'partial':
        return t('contextHome.announce.partial', 'Your context loaded, but some sections are unavailable.')
      case 'empty':
        return t('contextHome.announce.empty', 'Your context loaded and is empty.')
      default:
        return t('contextHome.announce.complete', 'Your context loaded.')
    }
  }, [kind, snapshot, refreshing, t])
}
