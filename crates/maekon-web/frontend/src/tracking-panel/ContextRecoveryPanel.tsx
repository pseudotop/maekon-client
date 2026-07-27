import { ArrowLeft, Check, Clock, FileSearch, Pencil, RefreshCw, ShieldAlert, X } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { type RecoveryResult, useContextRecovery } from '../hooks/useContextRecovery'
import { type TaskCandidateView, type TodoState, useDurableTasks } from '../hooks/useDurableTasks'

/**
 * Context recovery vertical (MK-CONTEXT-01.T03, #8892).
 *
 * The single shipped path: request a current-scene generation turn, review the
 * source-backed next step, edit/confirm/reject/retry, and — only on an explicit
 * confirm — turn it into a durable to-do whose lifecycle stays visible across a
 * restart. Lives in `tracking-panel/` so it renders on the always-available
 * floating panel without opening Chat, Search, or a dashboard. The card shows
 * only bounded provenance (source kind + lifecycle + capture time); raw OCR /
 * window text never reaches this surface.
 */

const LOCAL_SCENE_SOURCE = 'LOCAL_CURRENT_SCENE'

const TODO_NEXT_STATES: Record<TodoState, TodoState[]> = {
  CONFIRMED: ['IN_PROGRESS', 'DONE', 'CANCELLED'],
  IN_PROGRESS: ['WAITING', 'DONE', 'CANCELLED'],
  WAITING: ['IN_PROGRESS', 'DONE', 'CANCELLED'],
  DONE: [],
  CANCELLED: [],
}

const TODO_STATE_LABEL: Record<TodoState, { key: string; fallback: string }> = {
  CONFIRMED: { key: 'durableTasks.state.confirmed', fallback: 'Confirmed' },
  IN_PROGRESS: { key: 'durableTasks.state.inProgress', fallback: 'In progress' },
  WAITING: { key: 'durableTasks.state.waiting', fallback: 'Waiting' },
  DONE: { key: 'durableTasks.state.done', fallback: 'Done' },
  CANCELLED: { key: 'durableTasks.state.cancelled', fallback: 'Cancelled' },
}

interface OutcomeCopy {
  testid: string
  icon: React.ReactNode
  title: string
  detail: string
}

export function ContextRecoveryPanel({ onBack }: { onBack: () => void }) {
  const { t } = useTranslation()
  const { phase, result, error, generate } = useContextRecovery()
  const { candidates, todos, confirmCandidate, dismissCandidate, transitionTodo, refresh } = useDurableTasks()
  const [notice, setNotice] = useState<string | null>(null)
  const startedRef = useRef(false)

  const runGenerate = useCallback(async () => {
    setNotice(null)
    const res = await generate()
    // The backend ingests the reviewable candidate during the turn, so re-read
    // the durable store once generation settles.
    if (res?.status === 'generated') await refresh()
  }, [generate, refresh])

  useEffect(() => {
    if (startedRef.current) return
    startedRef.current = true
    void runGenerate()
  }, [runGenerate])

  const proposed = candidates.filter((c) => c.state === 'PROPOSED' && c.source_kind === LOCAL_SCENE_SOURCE)

  const handleConfirm = useCallback(
    async (candidate: TaskCandidateView, edit?: { title: string; body: string | null }) => {
      const outcome = await confirmCandidate(candidate.id, candidate.revision, {
        confirmedTitle: edit?.title ?? null,
        confirmedBody: edit?.body ?? null,
      })
      switch (outcome.outcome) {
        case 'confirmed':
        case 'already_transitioned':
          setNotice(t('recovery.confirmedNotice', 'Added to your to-dos'))
          break
        case 'revision_conflict':
          setNotice(t('recovery.conflictNotice', 'That item just changed — refreshed for you'))
          await refresh()
          break
        default:
          setNotice(t('recovery.confirmFailedNotice', "Couldn't confirm this step"))
          break
      }
    },
    [confirmCandidate, refresh, t],
  )

  const handleDismiss = useCallback(
    async (candidate: TaskCandidateView) => {
      await dismissCandidate(candidate.id, candidate.revision)
      setNotice(t('recovery.dismissedNotice', 'Dismissed'))
    },
    [dismissCandidate, t],
  )

  return (
    <section
      aria-label={t('recovery.title', 'Find my next step')}
      data-visual-region="context-recovery"
      className="flex max-h-[calc(100vh-2.5rem)] flex-col gap-2 overflow-y-auto px-3 pt-2 pb-3"
    >
      <header className="flex items-center gap-2">
        <button
          type="button"
          onClick={onBack}
          aria-label={t('recovery.back', 'Back')}
          className="rounded p-0.5 transition-colors hover:bg-white/20"
        >
          <ArrowLeft size={14} />
        </button>
        <h2 className="flex-1 truncate font-medium text-white/90 text-xs">
          {t('recovery.title', 'Find my next step')}
        </h2>
        <button
          type="button"
          onClick={() => void runGenerate()}
          disabled={phase === 'loading'}
          aria-label={t('recovery.retry', 'Try again')}
          data-testid="recovery-retry"
          className="rounded p-0.5 transition-colors hover:bg-white/20 disabled:opacity-40"
        >
          <RefreshCw size={13} />
        </button>
      </header>

      {notice && (
        <output aria-live="polite" data-testid="recovery-notice" className="px-1 text-[10px] text-white/70">
          {notice}
        </output>
      )}

      {(phase === 'idle' || phase === 'loading') && (
        <div
          data-testid="recovery-loading"
          aria-live="polite"
          className="flex items-center gap-2 px-1 py-3 text-white/70"
        >
          <RefreshCw size={13} className="animate-spin" />
          <span className="text-[11px]">{t('recovery.loading', 'Reading your current screen…')}</span>
        </div>
      )}

      {phase === 'error' && (
        <OutcomeBlock
          copy={{
            testid: 'recovery-outcome-error',
            icon: <ShieldAlert size={14} />,
            title: t('recovery.outcome.errorTitle', 'Something went wrong'),
            detail: error ?? t('recovery.outcome.errorDetail', 'The request could not be completed.'),
          }}
          retryLabel={t('recovery.retry', 'Try again')}
          onRetry={() => void runGenerate()}
        />
      )}

      {phase === 'done' && result && (
        <RecoveryOutcome
          result={result}
          proposed={proposed}
          onConfirm={handleConfirm}
          onDismiss={handleDismiss}
          onRetry={() => void runGenerate()}
        />
      )}

      <TodoList todos={todos} onTransition={transitionTodo} />
    </section>
  )
}

function RecoveryOutcome({
  result,
  proposed,
  onConfirm,
  onDismiss,
  onRetry,
}: {
  result: RecoveryResult
  proposed: TaskCandidateView[]
  onConfirm: (candidate: TaskCandidateView, edit?: { title: string; body: string | null }) => void
  onDismiss: (candidate: TaskCandidateView) => void
  onRetry: () => void
}) {
  const { t } = useTranslation()

  if (result.status === 'generated' && proposed.length > 0) {
    return (
      <div className="flex flex-col gap-2">
        {proposed.map((candidate) => (
          <RecoveryCard
            key={candidate.id}
            candidate={candidate}
            provenance={result.provenance}
            onConfirm={onConfirm}
            onDismiss={onDismiss}
          />
        ))}
      </div>
    )
  }

  const copy = outcomeCopy(result, t)
  return (
    <OutcomeBlock
      copy={copy}
      retryLabel={t('recovery.retry', 'Try again')}
      onRetry={onRetry}
      missing={outcomeMissing(result)}
    />
  )
}

function RecoveryCard({
  candidate,
  provenance,
  onConfirm,
  onDismiss,
}: {
  candidate: TaskCandidateView
  provenance: RecoveryResult['provenance']
  onConfirm: (candidate: TaskCandidateView, edit?: { title: string; body: string | null }) => void
  onDismiss: (candidate: TaskCandidateView) => void
}) {
  const { t } = useTranslation()
  const [editing, setEditing] = useState(false)
  const [draftTitle, setDraftTitle] = useState(candidate.title ?? '')
  const [draftBody, setDraftBody] = useState(candidate.body ?? '')

  const confirm = () => {
    if (editing) {
      onConfirm(candidate, { title: draftTitle, body: draftBody.trim() === '' ? null : draftBody })
    } else {
      onConfirm(candidate)
    }
  }

  return (
    <article
      data-testid="recovery-card"
      className="flex flex-col gap-1.5 rounded-lg border border-white/10 bg-white/5 px-2.5 py-2"
    >
      <div data-testid="recovery-source" className="flex flex-wrap items-center gap-1.5 text-[10px] text-white/55">
        <FileSearch size={11} />
        <span className="uppercase tracking-wide">{t('recovery.card.sourceLabel', 'Source')}</span>
        <span className="rounded bg-white/10 px-1 py-px text-white/80">
          {sourceKindLabel(candidate.source_kind, provenance, t)}
        </span>
        <span className="rounded bg-white/10 px-1 py-px text-white/70">
          {lifecycleLabel(candidate.source_lifecycle, t)}
        </span>
        {provenance && (
          <span className="flex items-center gap-0.5 text-white/45">
            <Clock size={10} />
            {t('recovery.card.captured', 'Captured {{when}}', { when: formatWhen(provenance.observed_at) })}
          </span>
        )}
      </div>

      <div className="text-[10px] text-white/40 uppercase tracking-wide">
        {t('recovery.card.nextStepLabel', 'Suggested next step')}
      </div>

      {editing ? (
        <div className="flex flex-col gap-1">
          <input
            aria-label={t('recovery.card.editTitle', 'Edit next step')}
            data-testid="recovery-edit-title"
            value={draftTitle}
            onChange={(e) => setDraftTitle(e.target.value)}
            className="w-full rounded border border-white/15 bg-black/40 px-1.5 py-1 text-[11px] text-white/90"
          />
          <textarea
            aria-label={t('recovery.card.editBody', 'Edit details')}
            data-testid="recovery-edit-body"
            value={draftBody}
            onChange={(e) => setDraftBody(e.target.value)}
            rows={2}
            className="w-full resize-none rounded border border-white/15 bg-black/40 px-1.5 py-1 text-[10px] text-white/80"
          />
        </div>
      ) : (
        <div data-testid="recovery-next-step" className="text-[11px] text-white/90">
          <div>{candidate.title ?? t('recovery.card.untitled', '(untitled step)')}</div>
          {candidate.body && <div className="mt-0.5 text-[10px] text-white/55">{candidate.body}</div>}
        </div>
      )}

      <div className="mt-0.5 flex flex-wrap items-center gap-1">
        <button
          type="button"
          onClick={confirm}
          data-testid="recovery-confirm"
          className="inline-flex items-center gap-1 rounded bg-status-connected/20 px-2 py-0.5 text-[10px] text-white/90 transition-colors hover:bg-status-connected/30"
        >
          <Check size={11} />
          {t('recovery.card.confirm', 'Add to to-dos')}
        </button>
        <button
          type="button"
          onClick={() => setEditing((v) => !v)}
          aria-pressed={editing}
          data-testid="recovery-edit"
          className="inline-flex items-center gap-1 rounded bg-white/10 px-2 py-0.5 text-[10px] text-white/80 transition-colors hover:bg-white/20"
        >
          <Pencil size={11} />
          {editing ? t('recovery.card.editing', 'Editing') : t('recovery.card.edit', 'Edit')}
        </button>
        <button
          type="button"
          onClick={() => onDismiss(candidate)}
          data-testid="recovery-dismiss"
          className="inline-flex items-center gap-1 rounded bg-white/10 px-2 py-0.5 text-[10px] text-white/70 transition-colors hover:bg-white/20"
        >
          <X size={11} />
          {t('recovery.card.dismiss', 'Dismiss')}
        </button>
      </div>
    </article>
  )
}

function OutcomeBlock({
  copy,
  retryLabel,
  onRetry,
  missing,
}: {
  copy: OutcomeCopy
  retryLabel: string
  onRetry: () => void
  missing?: string[]
}) {
  const { t } = useTranslation()
  return (
    <div
      data-testid={copy.testid}
      className="flex flex-col gap-1.5 rounded-lg border border-white/10 bg-white/5 px-2.5 py-2"
    >
      <div className="flex items-center gap-1.5 text-white/85">
        {copy.icon}
        <span className="font-medium text-[11px]">{copy.title}</span>
      </div>
      <p className="text-[10px] text-white/55">{copy.detail}</p>
      {missing && missing.length > 0 && (
        <ul className="flex flex-wrap gap-1">
          {missing.map((permission) => (
            <li key={permission} className="rounded bg-semantic-warning/15 px-1 py-px text-[9px] text-semantic-warning">
              {t('recovery.missingPermission', 'Needs: {{permission}}', { permission })}
            </li>
          ))}
        </ul>
      )}
      <button
        type="button"
        onClick={onRetry}
        className="mt-0.5 inline-flex items-center gap-1 self-start rounded bg-white/10 px-2 py-0.5 text-[10px] text-white/80 transition-colors hover:bg-white/20"
      >
        <RefreshCw size={11} />
        {retryLabel}
      </button>
    </div>
  )
}

function TodoList({
  todos,
  onTransition,
}: {
  todos: ReturnType<typeof useDurableTasks>['todos']
  onTransition: (id: string, target: TodoState, revision: number) => void
}) {
  const { t } = useTranslation()
  return (
    <section aria-labelledby="recovery-todos-heading" className="mt-1 border-white/10 border-t pt-1.5">
      <h3 id="recovery-todos-heading" className="px-1 pb-1 font-medium text-[10px] text-white/45">
        {t('recovery.todosHeading', 'Your confirmed to-dos')}
      </h3>
      {todos.length === 0 ? (
        <p className="px-1 text-[10px] text-white/40">{t('recovery.noTodos', 'No confirmed to-dos yet.')}</p>
      ) : (
        <ul className="flex flex-col gap-1">
          {todos.map((todo) => (
            <li
              key={todo.id}
              data-testid={`recovery-todo-${todo.id}`}
              className="flex flex-wrap items-center gap-1 rounded bg-white/5 px-1.5 py-1"
            >
              <span className="min-w-0 flex-1 truncate text-[11px] text-white/85">{todo.title}</span>
              <span className="rounded bg-white/10 px-1 py-px text-[9px] text-white/60">
                {t(TODO_STATE_LABEL[todo.state].key, TODO_STATE_LABEL[todo.state].fallback)}
              </span>
              {TODO_NEXT_STATES[todo.state].map((target) => (
                <button
                  key={target}
                  type="button"
                  onClick={() => onTransition(todo.id, target, todo.revision)}
                  className="rounded bg-white/10 px-1 py-px text-[9px] text-white/70 transition-colors hover:bg-white/20"
                >
                  {t(TODO_STATE_LABEL[target].key, TODO_STATE_LABEL[target].fallback)}
                </button>
              ))}
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

function outcomeMissing(result: RecoveryResult): string[] | undefined {
  return result.status === 'consent_required' ? result.missing_permissions : undefined
}

function outcomeCopy(result: RecoveryResult, t: ReturnType<typeof useTranslation>['t']): OutcomeCopy {
  switch (result.status) {
    case 'consent_required':
      return {
        testid: 'recovery-outcome-consent',
        icon: <ShieldAlert size={14} />,
        title: t('recovery.outcome.consentTitle', 'Screen access needed'),
        detail: t('recovery.outcome.consentDetail', 'Grant the listed permissions so the current screen can be read.'),
      }
    case 'throttled':
      return {
        testid: 'recovery-outcome-throttled',
        icon: <Clock size={14} />,
        title: t('recovery.outcome.throttledTitle', 'One at a time'),
        detail: t('recovery.outcome.throttledDetail', 'A request is already running, or today’s budget is used up.'),
      }
    case 'no_candidate':
      return {
        testid: 'recovery-outcome-no-candidate',
        icon: <FileSearch size={14} />,
        title: t('recovery.outcome.noCandidateTitle', 'No next step found'),
        detail: t('recovery.outcome.noCandidateDetail', 'Your screen was reviewed but no action was proposed.'),
      }
    default: {
      // analysis_unavailable — split provider outages from empty/insufficient context.
      const providerReasons = [
        'provider_unavailable',
        'provider_error',
        'provider_timeout',
        'active_session_unavailable',
      ]
      if (result.reason && providerReasons.includes(result.reason)) {
        return {
          testid: 'recovery-outcome-provider-offline',
          icon: <ShieldAlert size={14} />,
          title: t('recovery.outcome.providerOfflineTitle', 'Assistant offline'),
          detail: t('recovery.outcome.providerOfflineDetail', 'The suggestion provider is unavailable right now.'),
        }
      }
      return {
        testid: 'recovery-outcome-no-context',
        icon: <FileSearch size={14} />,
        title: t('recovery.outcome.noContextTitle', 'Not enough on screen'),
        detail: t('recovery.outcome.noContextDetail', "There wasn't enough current context to suggest a step."),
      }
    }
  }
}

function sourceKindLabel(
  kind: string,
  provenance: RecoveryResult['provenance'],
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (kind === LOCAL_SCENE_SOURCE || provenance?.source_id) {
    return t('recovery.source.localScene', 'Current screen')
  }
  return kind
}

function lifecycleLabel(lifecycle: string, t: ReturnType<typeof useTranslation>['t']): string {
  switch (lifecycle) {
    case 'ACTIVE':
      return t('recovery.lifecycle.active', 'Live')
    case 'DELETED':
      return t('recovery.lifecycle.deleted', 'Source deleted')
    case 'ACCESS_REVOKED':
      return t('recovery.lifecycle.revoked', 'Access revoked')
    case 'RETENTION_EXPIRED':
      return t('recovery.lifecycle.expired', 'Retention expired')
    default:
      return lifecycle
  }
}

function formatWhen(iso: string): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return iso
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}
