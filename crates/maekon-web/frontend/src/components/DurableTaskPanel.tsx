import { useTranslation } from 'react-i18next'
import {
  type TaskAction,
  type TaskOutcome,
  type TodoState,
  taskActionFailureKey,
  useDurableTasks,
} from '../hooks/useDurableTasks'
import { addErrorToast } from '../hooks/useToast'
import { useErrorDisplay } from '../i18n/errorDisplay'
import { motion, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { SupportDetails } from './SupportDetails'

/**
 * Durable task panel (ADR-028, #8577; mounted on the /tasks route by #8892).
 *
 * Shows reviewable candidates and confirmed to-dos. Seeing a suggestion never
 * creates a task; only the explicit Confirm action does. Every button carries
 * the entity's current revision so the underlying compare-and-swap rejects a
 * stale double-click instead of duplicating work.
 */

const NEXT_STATES: Record<TodoState, TodoState[]> = {
  CONFIRMED: ['IN_PROGRESS', 'WAITING', 'DONE', 'CANCELLED'],
  IN_PROGRESS: ['WAITING', 'DONE', 'CANCELLED'],
  WAITING: ['IN_PROGRESS', 'DONE', 'CANCELLED'],
  DONE: [],
  CANCELLED: [],
}

const STATE_LABEL: Record<TodoState, { key: string; fallback: string }> = {
  CONFIRMED: { key: 'durableTasks.state.confirmed', fallback: 'Confirmed' },
  IN_PROGRESS: { key: 'durableTasks.state.inProgress', fallback: 'In progress' },
  WAITING: { key: 'durableTasks.state.waiting', fallback: 'Waiting' },
  DONE: { key: 'durableTasks.state.done', fallback: 'Done' },
  CANCELLED: { key: 'durableTasks.state.cancelled', fallback: 'Cancelled' },
}

const ACTION_BUTTON = cn(
  'rounded-md border border-border bg-surface-base px-2 py-1 text-content-secondary text-xs hover:bg-surface-muted',
  motion.colors,
)

export function DurableTaskPanel() {
  const { t } = useTranslation()
  const { candidates, todos, loading, error, refresh, confirmCandidate, dismissCandidate, transitionTodo } =
    useDurableTasks()
  const describeError = useErrorDisplay()
  const reportActionFailure = (cause: unknown) => {
    addErrorToast(describeError(cause), undefined, { cause })
  }
  const reportOutcome = (outcome: TaskOutcome, action: TaskAction) => {
    const key = taskActionFailureKey(outcome, action)
    if (key)
      addErrorToast(describeError(outcome.outcome, t(key)), undefined, { cause: outcome.outcome, fallbackKey: key })
  }

  const stateLabel = (state: TodoState) => t(STATE_LABEL[state].key, STATE_LABEL[state].fallback)

  if (loading) {
    return (
      <div className="durable-tasks text-content-secondary text-sm" aria-busy="true">
        {t('durableTasks.loading', 'Loading tasks…')}
      </div>
    )
  }

  if (error) {
    return (
      <div className="durable-tasks__error rounded-md bg-semantic-error/10 px-3 py-2 text-semantic-error text-sm">
        <p role="alert">{error.text}</p>
        <SupportDetails detail={error.detail} />
        <button
          type="button"
          className={ACTION_BUTTON}
          data-testid="durable-tasks-retry"
          onClick={() => void refresh()}
        >
          {t('common.retry')}
        </button>
      </div>
    )
  }

  return (
    <div className="durable-tasks flex flex-col gap-6">
      <section aria-labelledby="durable-candidates-heading" className="flex flex-col gap-2">
        <h2 id="durable-candidates-heading" className={cn(typography.weight.medium, 'text-content text-sm')}>
          {t('durableTasks.suggestedHeading', 'Suggested tasks')}
        </h2>
        {candidates.length === 0 ? (
          <p className="durable-tasks__empty text-content-tertiary text-sm">
            {t('durableTasks.noSuggestions', 'No task suggestions to review.')}
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {candidates.map((candidate) => (
              <li
                key={candidate.id}
                data-testid={`candidate-${candidate.id}`}
                className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface-elevated px-3 py-2"
              >
                <span className="durable-tasks__title min-w-0 flex-1 text-content text-sm">
                  {candidate.title ?? t('durableTasks.noTitle', '(no title)')}
                </span>
                <button
                  type="button"
                  className={ACTION_BUTTON}
                  onClick={() =>
                    void confirmCandidate(candidate.id, candidate.revision)
                      .then((outcome) => reportOutcome(outcome, 'confirm'))
                      .catch(reportActionFailure)
                  }
                >
                  {t('durableTasks.confirm', 'Confirm')}
                </button>
                <button
                  type="button"
                  className={ACTION_BUTTON}
                  onClick={() =>
                    void dismissCandidate(candidate.id, candidate.revision)
                      .then((outcome) => reportOutcome(outcome, 'dismiss'))
                      .catch(reportActionFailure)
                  }
                >
                  {t('durableTasks.dismiss', 'Dismiss')}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section aria-labelledby="durable-todos-heading" className="flex flex-col gap-2">
        <h2 id="durable-todos-heading" className={cn(typography.weight.medium, 'text-content text-sm')}>
          {t('durableTasks.todosHeading', 'To-dos')}
        </h2>
        {todos.length === 0 ? (
          <p className="durable-tasks__empty text-content-tertiary text-sm">
            {t('durableTasks.noTodos', 'No confirmed to-dos yet.')}
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {todos.map((todo) => (
              <li
                key={todo.id}
                data-testid={`todo-${todo.id}`}
                className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface-elevated px-3 py-2"
              >
                <span className="durable-tasks__title min-w-0 flex-1 text-content text-sm">{todo.title}</span>
                <span className="durable-tasks__state rounded bg-surface-muted px-2 py-0.5 text-content-secondary text-xs">
                  {stateLabel(todo.state)}
                </span>
                {NEXT_STATES[todo.state].map((target) => (
                  <button
                    key={target}
                    type="button"
                    className={ACTION_BUTTON}
                    onClick={() =>
                      void transitionTodo(todo.id, target, todo.revision)
                        .then((outcome) => reportOutcome(outcome, target))
                        .catch(reportActionFailure)
                    }
                  >
                    {stateLabel(target)}
                  </button>
                ))}
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  )
}
