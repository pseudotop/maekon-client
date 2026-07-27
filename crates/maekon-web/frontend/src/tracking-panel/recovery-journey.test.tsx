import { act, fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { App } from './App'

/**
 * Black-box acceptance journey for the current-context recovery vertical
 * (MK-CONTEXT-01.T03, #8892).
 *
 * This drives the REAL shipped tracking-panel App from its authoritative entry
 * control through the whole path — entry → generated card → visible source →
 * edit/confirm → durable TODO → restart/resume — against a stateful FAKE of the
 * Tauri backend. It is a mocked shipped-flow test (no real desktop capture); the
 * real-capture demo is #8686's human evidence and is out of scope here. It also
 * proves the outcome states, the "viewing never creates a TODO" invariant, and
 * confirm idempotency. Unlike an isolated component test, it exercises the actual
 * Tracking Panel control that invokes `request_current_context_suggestions`.
 */

const mockEmit = vi.fn()
const mockListen = vi.fn()
const mockInvoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: (...args: unknown[]) => mockEmit(...args),
  listen: (...args: unknown[]) => mockListen(...args),
}))
vi.mock('@tauri-apps/api/window', () => ({
  currentMonitor: () =>
    Promise.resolve({ workArea: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1040 } } }),
  getCurrentWindow: () => ({
    outerPosition: () => Promise.resolve({ x: 0, y: 0 }),
    scaleFactor: () => Promise.resolve(1),
    setPosition: () => Promise.resolve(undefined),
    setSize: () => Promise.resolve(undefined),
    startDragging: vi.fn(),
  }),
}))
vi.mock('@tauri-apps/api/dpi', () => ({
  LogicalPosition: class {
    constructor(
      readonly x: number,
      readonly y: number,
    ) {}
  },
  LogicalSize: class {
    constructor(
      readonly width: number,
      readonly height: number,
    ) {}
  },
}))

// ── Stateful fake of the durable-task + current-context backend ──────────────
// Persists across App remounts within a test, so an unmount/remount models the
// SQLite store surviving a client restart.
interface FakeCandidate {
  id: string
  state: string
  title: string | null
  body: string | null
  proposed_due: null
  expires_at: string
  source_kind: string
  source_lifecycle: string
  revision: number
}
interface FakeTodo {
  id: string
  state: string
  title: string
  body: string | null
  due: null
  revision: number
  created_at: string
  updated_at: string
}

let store: { candidates: FakeCandidate[]; todos: FakeTodo[]; confirmCalls: Record<string, unknown>[]; genCount: number }
let recoveryOutcome: Record<string, unknown>

function resetStore() {
  store = { candidates: [], todos: [], confirmCalls: [], genCount: 0 }
  recoveryOutcome = {
    status: 'generated',
    reason: null,
    admitted_count: 1,
    queue_count: 1,
    admitted_suggestion_ids: ['sugg-1'],
    missing_permissions: [],
    provenance: {
      source_id: 'local_current_scene',
      observed_at: '2026-07-19T04:50:00Z',
      captured_at: '2026-07-19T04:50:01Z',
    },
  }
}

function installBackend() {
  mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'get_capture_status':
        return Promise.resolve({ paused: false, indicator_visible: true, consent_granted: true, permitted: true })
      case 'get_connection_status':
        return Promise.resolve({ server: true, llm: true, cli: true })
      case 'get_pending_suggestion_count':
        return Promise.resolve(0)
      case 'get_panel_position':
        return Promise.resolve(null)
      case 'request_current_context_suggestions': {
        if (recoveryOutcome.status === 'generated') {
          store.genCount += 1
          store.candidates.push({
            id: `tcand_gen${store.genCount}`,
            state: 'PROPOSED',
            title: 'Reply to the design review thread',
            body: 'Two open comments are waiting on you.',
            proposed_due: null,
            expires_at: '2026-08-01T00:00:00Z',
            source_kind: 'LOCAL_CURRENT_SCENE',
            source_lifecycle: 'ACTIVE',
            revision: 1,
          })
        }
        return Promise.resolve(recoveryOutcome)
      }
      case 'list_task_candidates':
        return Promise.resolve(store.candidates.filter((c) => c.state === 'PROPOSED'))
      case 'list_todos':
        return Promise.resolve(store.todos)
      case 'confirm_task_candidate': {
        store.confirmCalls.push(args ?? {})
        const id = args?.candidateId as string
        const candidate = store.candidates.find((c) => c.id === id)
        // Idempotent: an already-confirmed candidate never mints a second to-do.
        if (!candidate || candidate.state !== 'PROPOSED') {
          return Promise.resolve({ outcome: 'already_transitioned', current_state: 'CONFIRMED' })
        }
        candidate.state = 'CONFIRMED'
        const todoTitle = (args?.confirmedTitle as string | null) ?? candidate.title ?? '(no title)'
        store.todos.push({
          id: `todo_for_${id}`,
          state: 'CONFIRMED',
          title: todoTitle,
          body: (args?.confirmedBody as string | null) ?? candidate.body,
          due: null,
          revision: 1,
          created_at: '2026-07-20T00:00:00Z',
          updated_at: '2026-07-20T00:00:00Z',
        })
        return Promise.resolve({ outcome: 'confirmed', candidate_id: id, todo_id: `todo_for_${id}`, revision: 2 })
      }
      case 'dismiss_task_candidate': {
        const id = args?.candidateId as string
        const candidate = store.candidates.find((c) => c.id === id)
        if (candidate) candidate.state = 'DISMISSED'
        return Promise.resolve({ outcome: 'dismissed', candidate_id: id, revision: 2 })
      }
      default:
        return Promise.resolve(undefined)
    }
  })
}

async function openRecovery() {
  // Expand the collapsed floating bar (toggle is icon-only, labelled by title),
  // then take the authoritative recovery entry. The toggle handler is async
  // (window resize), so flush it under act() like the panel's own tests do.
  await act(async () => {
    fireEvent.click(screen.getByTitle('Expand'))
  })
  await screen.findByRole('region', { name: 'Floating command menu' })
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Find my next step' }))
  })
}

beforeEach(() => {
  resetStore()
  mockEmit.mockReset().mockResolvedValue(undefined)
  mockListen.mockReset().mockResolvedValue(() => {})
  mockInvoke.mockReset()
  installBackend()
  // The hooks invoke via dynamic `await import('@tauri-apps/api/core')`, whose
  // real `invoke` delegates to this global. Set it so every hook's invoke —
  // including the nested useDurableTasks reads — reaches the fake backend.
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: (...args: unknown[]) => mockInvoke(...args),
  }
})

afterEach(() => {
  vi.clearAllMocks()
})

describe('context recovery journey (#8892)', () => {
  it('entry → generated card → visible source → edit/confirm → durable TODO → restart/resume', async () => {
    const view = renderWithProviders(<App />)
    await openRecovery()

    // Generated card with a visible source/provenance and the proposed next step.
    const card = await screen.findByTestId('recovery-card')
    expect(within(card).getByTestId('recovery-source')).toBeInTheDocument()
    expect(within(card).getByTestId('recovery-next-step')).toHaveTextContent('Reply to the design review thread')

    // Invariant: viewing must not create a to-do yet.
    expect(store.todos).toHaveLength(0)
    expect(screen.getByText('No confirmed to-dos yet.')).toBeInTheDocument()
    expect(mockInvoke.mock.calls.some((c) => c[0] === 'confirm_task_candidate')).toBe(false)

    // The entry drove the real generation use case, not scene analysis / the queue.
    expect(mockInvoke.mock.calls.some((c) => c[0] === 'request_current_context_suggestions')).toBe(true)
    expect(mockInvoke.mock.calls.some((c) => c[0] === 'analyze_current_scene')).toBe(false)

    // Edit the next step inline, then confirm.
    fireEvent.click(within(card).getByTestId('recovery-edit'))
    const titleInput = within(card).getByTestId('recovery-edit-title')
    fireEvent.change(titleInput, { target: { value: 'Reply to Dana about the review' } })
    fireEvent.click(within(card).getByTestId('recovery-confirm'))

    // Exactly one durable to-do, carrying the edited title, on the visible surface.
    await waitFor(() => expect(screen.getByTestId('recovery-todo-todo_for_tcand_gen1')).toBeInTheDocument())
    expect(screen.getByTestId('recovery-todo-todo_for_tcand_gen1')).toHaveTextContent('Reply to Dana about the review')
    expect(store.todos).toHaveLength(1)

    // Confirm echoed the candidate revision + an idempotency key (CAS inputs).
    const confirmArgs = store.confirmCalls[0]
    expect(confirmArgs.candidateId).toBe('tcand_gen1')
    expect(confirmArgs.expectedRevision).toBe(1)
    expect(typeof confirmArgs.idempotencyKey).toBe('string')
    expect(confirmArgs.confirmedTitle).toBe('Reply to Dana about the review')

    // Restart/resume: unmount the whole app, remount, and the confirmed to-do
    // is restored from the persisted store.
    view.unmount()
    renderWithProviders(<App />)
    await openRecovery()
    await waitFor(() => expect(screen.getByTestId('recovery-todo-todo_for_tcand_gen1')).toBeInTheDocument())
    expect(store.todos).toHaveLength(1)
  })

  it('double-confirm creates exactly one durable TODO (idempotent)', async () => {
    renderWithProviders(<App />)
    await openRecovery()
    const card = await screen.findByTestId('recovery-card')

    // Fire confirm twice before the list settles.
    fireEvent.click(within(card).getByTestId('recovery-confirm'))
    fireEvent.click(within(card).getByTestId('recovery-confirm'))

    await waitFor(() => expect(screen.getByTestId('recovery-todo-todo_for_tcand_gen1')).toBeInTheDocument())
    expect(store.todos).toHaveLength(1)
  })

  it('surfaces consent-required without generating a candidate', async () => {
    recoveryOutcome = {
      status: 'consent_required',
      reason: 'screen_capture_consent_required',
      admitted_count: 0,
      queue_count: 0,
      admitted_suggestion_ids: [],
      missing_permissions: ['screen_capture'],
      provenance: null,
    }
    renderWithProviders(<App />)
    await openRecovery()

    expect(await screen.findByTestId('recovery-outcome-consent')).toBeInTheDocument()
    expect(screen.queryByTestId('recovery-card')).not.toBeInTheDocument()
    expect(store.todos).toHaveLength(0)
  })

  it('surfaces provider-offline as a distinct degraded state', async () => {
    recoveryOutcome = {
      status: 'analysis_unavailable',
      reason: 'provider_unavailable',
      admitted_count: 0,
      queue_count: 0,
      admitted_suggestion_ids: [],
      missing_permissions: [],
      provenance: null,
    }
    renderWithProviders(<App />)
    await openRecovery()

    expect(await screen.findByTestId('recovery-outcome-provider-offline')).toBeInTheDocument()
  })

  it('surfaces no-candidate when generation proposes nothing', async () => {
    recoveryOutcome = {
      status: 'no_candidate',
      reason: 'no_valid_candidate',
      admitted_count: 0,
      queue_count: 0,
      admitted_suggestion_ids: [],
      missing_permissions: [],
      provenance: null,
    }
    renderWithProviders(<App />)
    await openRecovery()

    expect(await screen.findByTestId('recovery-outcome-no-candidate')).toBeInTheDocument()
  })
})
