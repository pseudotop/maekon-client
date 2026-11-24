import { act, fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { clearToasts } from '../hooks/useToast'
import i18n from '../i18n'
import en from '../i18n/locales/en.json'
import ko from '../i18n/locales/ko.json'
import { DurableTaskPanel } from './DurableTaskPanel'
import { ToastContainer } from './ui/Toast'

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

const candidate = {
  id: 'tcand_1',
  state: 'PROPOSED',
  title: 'Follow up on the report',
  body: null,
  proposed_due: null,
  expires_at: '2026-08-01T00:00:00Z',
  source_kind: 'LOCAL_CURRENT_SCENE',
  source_lifecycle: 'ACTIVE',
  revision: 1,
}

const todo = {
  id: 'todo_1',
  state: 'CONFIRMED',
  title: 'Prepare the deck',
  body: null,
  due: null,
  revision: 1,
  created_at: '2026-07-20T00:00:00Z',
  updated_at: '2026-07-20T00:00:00Z',
}

function primeLists(candidates: unknown[], todos: unknown[]) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'list_task_candidates') return Promise.resolve(candidates)
    if (cmd === 'list_todos') return Promise.resolve(todos)
    return Promise.resolve({ outcome: 'confirmed' })
  })
}

describe('DurableTaskPanel', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    mockInvoke.mockReset()
    // The real @tauri-apps/api/core `invoke` delegates to this global.
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  afterEach(() => {
    act(() => clearToasts())
  })

  it('renders reviewable candidates and confirmed to-dos', async () => {
    primeLists([candidate], [todo])
    renderWithProviders(<DurableTaskPanel />)

    await waitFor(() => {
      expect(screen.getByText('Follow up on the report')).toBeInTheDocument()
    })
    expect(screen.getByText('Prepare the deck')).toBeInTheDocument()
    // A confirmed to-do offers only forward transitions, never a reopen.
    expect(screen.getByRole('button', { name: 'In progress' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Confirmed' })).not.toBeInTheDocument()
  })

  it('confirms a candidate with its current revision and an idempotency key', async () => {
    primeLists([candidate], [])
    renderWithProviders(<DurableTaskPanel />)

    await waitFor(() => {
      expect(screen.getByTestId('candidate-tcand_1')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))

    await waitFor(() => {
      const confirmCall = mockInvoke.mock.calls.find((c) => c[0] === 'confirm_task_candidate')
      expect(confirmCall).toBeDefined()
      const args = confirmCall?.[1] as Record<string, unknown>
      expect(args.candidateId).toBe('tcand_1')
      expect(args.expectedRevision).toBe(1)
      expect(typeof args.idempotencyKey).toBe('string')
      expect((args.idempotencyKey as string).length).toBeGreaterThan(0)
    })
  })

  it('shows an empty-state message when there is nothing to review', async () => {
    primeLists([], [])
    renderWithProviders(<DurableTaskPanel />)

    await waitFor(() => {
      expect(screen.getByText('No task suggestions to review.')).toBeInTheDocument()
    })
    expect(screen.getByText('No confirmed to-dos yet.')).toBeInTheDocument()
  })

  it('offers a localized retry after loading fails, without claiming the store is empty', async () => {
    mockInvoke.mockRejectedValue(new Error('fixture diagnostic'))
    const { container } = renderWithProviders(<DurableTaskPanel />)
    expect(await screen.findByRole('alert')).toHaveTextContent(en.durableTasks.loadError)
    expect(container.querySelector('.durable-tasks__empty')).toBeNull()
    expect(screen.getByTestId('support-details')).not.toHaveAttribute('open')
    const calls = mockInvoke.mock.calls.length
    await act(async () => {
      await i18n.changeLanguage('ko')
    })
    expect(screen.getByRole('alert')).toHaveTextContent(ko.durableTasks.loadError)
    expect(mockInvoke).toHaveBeenCalledTimes(calls)
    primeLists([candidate], [todo])
    fireEvent.click(screen.getByTestId('durable-tasks-retry'))
    expect(await screen.findByTestId('candidate-tcand_1')).toBeInTheDocument()
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.queryByTestId('support-details')).toBeNull()
  })

  it.each([
    ['confirm', 'confirm_task_candidate', 'candidate-tcand_1', 0],
    ['dismiss', 'dismiss_task_candidate', 'candidate-tcand_1', 1],
    ['transition', 'transition_todo', 'todo-todo_1', 0],
  ] as const)('reports a rejected %s action without losing the existing task', async (_action, command, rowId, index) => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_task_candidates') return [candidate]
      if (cmd === 'list_todos') return [todo]
      if (cmd === command) throw { code: 'future.task_failure', message: 'fixture diagnostic' }
      throw new Error(`unexpected test command: ${cmd}`)
    })
    renderWithProviders(
      <>
        <DurableTaskPanel />
        <ToastContainer />
      </>,
    )
    const row = await screen.findByTestId(rowId)
    fireEvent.click(within(row).getAllByRole('button')[index])
    await waitFor(() => expect(document.querySelector('[aria-live="polite"] p')).toHaveTextContent(en.errors.generic))
    expect(screen.getByTestId('support-details-text')).toHaveTextContent('future.task_failure: fixture diagnostic')
    expect(screen.getByTestId('support-details')).not.toHaveAttribute('open')
    expect(screen.getByTestId(rowId)).toBeInTheDocument()
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === command)).toHaveLength(1)
  })

  it.each([
    ['confirm_task_candidate', 'candidate-tcand_1', 0, { outcome: 'consent_required' }, 'actionConsentRequired'],
    ['dismiss_task_candidate', 'candidate-tcand_1', 1, { outcome: 'revision_conflict' }, 'actionConflict'],
    ['transition_todo', 'todo-todo_1', 0, { outcome: 'source_unavailable' }, 'actionSourceUnavailable'],
    [
      'dismiss_task_candidate',
      'candidate-tcand_1',
      1,
      { outcome: 'already_transitioned', current_state: 'CONFIRMED' },
      'actionConflict',
    ],
    ['confirm_task_candidate', 'candidate-tcand_1', 0, { outcome: 'idempotency_mismatch' }, 'actionFailed'],
    ['transition_todo', 'todo-todo_1', 0, { outcome: 'future_outcome' }, 'actionFailed'],
  ] as const)('reports a domain refusal from %s (%j)', async (command, rowId, index, outcome, key) => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_task_candidates') return [candidate]
      if (cmd === 'list_todos') return [todo]
      if (cmd === command) return outcome
      throw new Error(`unexpected test command: ${cmd}`)
    })
    renderWithProviders(
      <>
        <DurableTaskPanel />
        <ToastContainer />
      </>,
    )
    const row = await screen.findByTestId(rowId)
    fireEvent.click(within(row).getAllByRole('button')[index])
    await waitFor(() =>
      expect(document.querySelector('[aria-live="polite"] p')).toHaveTextContent(i18n.t(`durableTasks.${key}`)),
    )
    expect(screen.getByTestId(rowId)).toBeInTheDocument()
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === command)).toHaveLength(1)
    const calls = mockInvoke.mock.calls.length
    await act(async () => {
      await i18n.changeLanguage('ko')
    })
    expect(document.querySelector('[aria-live="polite"] p')).toHaveTextContent(i18n.t(`durableTasks.${key}`))
    expect(mockInvoke).toHaveBeenCalledTimes(calls)
    expect(screen.getAllByTestId('support-details')).toHaveLength(1)
    expect(screen.getByTestId('support-details')).not.toHaveAttribute('open')
  })

  it.each([
    ['confirm_task_candidate', 'candidate-tcand_1', 0, { outcome: 'confirmed' }],
    ['confirm_task_candidate', 'candidate-tcand_1', 0, { outcome: 'already_transitioned', current_state: 'CONFIRMED' }],
    ['dismiss_task_candidate', 'candidate-tcand_1', 1, { outcome: 'dismissed' }],
    ['dismiss_task_candidate', 'candidate-tcand_1', 1, { outcome: 'already_transitioned', current_state: 'DISMISSED' }],
    ['transition_todo', 'todo-todo_1', 0, { outcome: 'transitioned', state: 'IN_PROGRESS' }],
    ['transition_todo', 'todo-todo_1', 0, { outcome: 'already_transitioned', current_state: 'IN_PROGRESS' }],
  ] as const)('preserves applied and idempotent %s outcomes (%j)', async (command, rowId, index, outcome) => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_task_candidates') return [candidate]
      if (cmd === 'list_todos') return [todo]
      if (cmd === command) return outcome
      throw new Error(`unexpected test command: ${cmd}`)
    })
    renderWithProviders(
      <>
        <DurableTaskPanel />
        <ToastContainer />
      </>,
    )
    fireEvent.click(within(await screen.findByTestId(rowId)).getAllByRole('button')[index])
    await waitFor(() => expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'list_todos')).toHaveLength(2))
    expect(screen.getByTestId(rowId)).toBeInTheDocument()
    expect(document.querySelector('[aria-live="polite"] p')).toBeNull()
  })
})
