import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { DurableTaskPanel } from './DurableTaskPanel'

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
  beforeEach(() => {
    mockInvoke.mockReset()
    // The real @tauri-apps/api/core `invoke` delegates to this global.
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
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
})
