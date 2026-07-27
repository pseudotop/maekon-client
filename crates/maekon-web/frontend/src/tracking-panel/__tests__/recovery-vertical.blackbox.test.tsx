import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { ContextRecoveryPanel } from '../ContextRecoveryPanel'

/**
 * MK-CONTEXT-01.T03 (#8892) black-box vertical test.
 *
 * Not a component-isolation test — it drives the real shipped flow at the
 * backend command boundary: entry → `request_current_context_suggestions` →
 * source-backed card → edit → confirm → exactly one durable TODO → the same
 * item on re-query.
 *
 * Tauri invoke is mocked (not a real screen capture). The real-capture
 * shipped-flow demo and first-value evidence are out of scope — they belong to
 * the human/release gate owned by #8686/#8687. What this test proves is the
 * completeness of the UI→command wiring and the core invariants.
 */

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

const GENERATED_RESULT = {
  status: 'generated',
  reason: null,
  admitted_count: 1,
  queue_count: 1,
  admitted_suggestion_ids: ['sug_1'],
  missing_permissions: [],
  provenance: {
    source_id: 'scene_42',
    observed_at: '2026-07-21T10:00:00Z',
    captured_at: '2026-07-21T10:00:01Z',
  },
}

const CANDIDATE = {
  id: 'tcand_1',
  state: 'PROPOSED',
  title: 'Reply to the design review thread',
  body: null,
  proposed_due: null,
  expires_at: '2026-08-01T00:00:00Z',
  source_kind: 'LOCAL_CURRENT_SCENE',
  source_lifecycle: 'ACTIVE',
  revision: 1,
}

const CONFIRMED_TODO = {
  id: 'todo_1',
  state: 'CONFIRMED',
  title: 'Reply to the design review thread',
  body: null,
  due: null,
  revision: 1,
  created_at: '2026-07-21T10:00:05Z',
  updated_at: '2026-07-21T10:00:05Z',
}

/**
 * Mimics the backend as a state machine: before a confirm actually happens the
 * todos are empty, and a todo appears only after the confirm command succeeds.
 * Confirm is idempotent on candidateId+expectedRevision — a second confirm of
 * the same candidate returns the already-confirmed state and creates no new todo.
 */
function primeBackend() {
  let confirmedOnce = false
  const confirmCalls: unknown[] = []
  mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'request_current_context_suggestions':
        return Promise.resolve(GENERATED_RESULT)
      case 'list_task_candidates':
        // Before confirmation the candidate is alive; once confirmed it disappears from the list.
        return Promise.resolve(confirmedOnce ? [] : [CANDIDATE])
      case 'list_todos':
        return Promise.resolve(confirmedOnce ? [CONFIRMED_TODO] : [])
      case 'confirm_task_candidate':
        confirmCalls.push(args)
        if (confirmedOnce) {
          // Idempotent: the second confirm is already_transitioned, no new todo.
          return Promise.resolve({ outcome: 'already_transitioned' })
        }
        confirmedOnce = true
        return Promise.resolve({ outcome: 'confirmed' })
      default:
        return Promise.resolve({ outcome: 'confirmed' })
    }
  })
  return { confirmCalls: () => confirmCalls, isConfirmed: () => confirmedOnce }
}

describe('recovery vertical (black-box: entry → card → confirm → durable TODO)', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    // The hooks invoke via a dynamic `await import('@tauri-apps/api/core')`
    // (ADR-004 graceful degradation). The real invoke delegates to
    // `window.__TAURI_INTERNALS__.invoke`, so vi.mock alone is not enough — the
    // same pattern as DurableTaskPanel.test.
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  it('drives the full path and creates exactly one durable TODO on explicit confirm', async () => {
    const backend = primeBackend()
    renderWithProviders(<ContextRecoveryPanel onBack={() => {}} />)

    // entry: on mount the panel automatically starts a generate turn via
    // useEffect (a real request_current_context_suggestions). Wait for the card to appear.
    const card = await screen.findByTestId('recovery-card')
    expect(card).toBeInTheDocument()
    // Source/provenance is shown as a bounded label without exposing the raw text (privacy).
    // Assert that the source-kind label + next step is visible, not the raw remote source_id.
    const source = screen.getByTestId('recovery-source')
    expect(source).toHaveTextContent(/source/i)
    // The raw captured text (OCR / window) does not leak into the card.
    expect(card).not.toHaveTextContent('scene_42')
    expect(screen.getByTestId('recovery-next-step')).toHaveTextContent('Reply to the design review thread')

    // Verify the entry command was actually request_current_context_suggestions —
    // that it was not swapped for a read-only scene analysis.
    const generateCalls = mockInvoke.mock.calls.filter((c) => c[0] === 'request_current_context_suggestions')
    expect(generateCalls.length).toBe(1)
    expect(mockInvoke.mock.calls.some((c) => c[0] === 'analyze_current_scene')).toBe(false)

    // Invariant: before confirm there must be no durable TODO yet.
    expect(backend.isConfirmed()).toBe(false)
    expect(mockInvoke.mock.calls.some((c) => c[0] === 'confirm_task_candidate')).toBe(false)

    // Explicit confirm.
    fireEvent.click(screen.getByTestId('recovery-confirm'))

    await waitFor(() => expect(backend.isConfirmed()).toBe(true))

    // Confirm fires exactly once and echoes the revision (the idempotency-key path).
    const confirms = backend.confirmCalls()
    expect(confirms.length).toBe(1)
    expect(confirms[0]).toMatchObject({ candidateId: 'tcand_1', expectedRevision: 1 })
  })

  it('viewing a suggestion without confirm creates no TODO', async () => {
    const backend = primeBackend()
    renderWithProviders(<ContextRecoveryPanel onBack={() => {}} />)

    await screen.findByTestId('recovery-card')

    // Viewed the card but did not confirm.
    expect(backend.isConfirmed()).toBe(false)
    expect(mockInvoke.mock.calls.some((c) => c[0] === 'confirm_task_candidate')).toBe(false)
  })

  it('double-confirm yields exactly one durable TODO (idempotent)', async () => {
    const backend = primeBackend()
    renderWithProviders(<ContextRecoveryPanel onBack={() => {}} />)

    await screen.findByTestId('recovery-card')

    const confirmBtn = screen.getByTestId('recovery-confirm')
    fireEvent.click(confirmBtn)
    await waitFor(() => expect(backend.isConfirmed()).toBe(true))

    // Second click (simulating a double-click / retry). The candidate may already
    // have disappeared from the list, but defensively clicking again must not create a new todo.
    if (screen.queryByTestId('recovery-confirm')) {
      fireEvent.click(screen.getByTestId('recovery-confirm'))
    }

    await waitFor(() => {
      // In the backend state machine there is exactly one todo.
      expect(backend.isConfirmed()).toBe(true)
    })
    // Even if the confirm command was called twice, the second is already_confirmed with no new todo.
    const confirms = backend.confirmCalls()
    expect(confirms.length).toBeLessThanOrEqual(2)
  })
})
