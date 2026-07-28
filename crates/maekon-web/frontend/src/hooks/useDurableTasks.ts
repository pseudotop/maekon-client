import { useCallback, useEffect, useRef, useState } from 'react'

/**
 * Durable task lifecycle hook (ADR-028, #8577).
 *
 * Loads reviewable candidates + confirmed to-dos over the Tauri IPC surface and
 * exposes confirm/dismiss/transition actions. Every mutation carries an
 * idempotency key and the expected revision so a retry or a double-click can
 * never create a second to-do or skip a compare-and-swap. The WebView never
 * supplies a source reference, todo id, or receipt — those are minted server
 * side.
 */

export type CandidateState = 'PROPOSED' | 'CONFIRMED' | 'DISMISSED' | 'EXPIRED'
export type TodoState = 'CONFIRMED' | 'IN_PROGRESS' | 'WAITING' | 'DONE' | 'CANCELLED'

export interface TaskCandidateView {
  id: string
  state: CandidateState
  title: string | null
  body: string | null
  proposed_due: string | null
  expires_at: string
  source_kind: string
  source_lifecycle: string
  revision: number
}

export interface TodoView {
  id: string
  state: TodoState
  title: string
  body: string | null
  due: string | null
  revision: number
  created_at: string
  updated_at: string
}

export interface TaskOutcome {
  outcome: string
  [key: string]: unknown
}

/** Lazy-loaded invoke — graceful degradation outside Tauri (ADR-004). */
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: inv } = await import('@tauri-apps/api/core')
  return inv<T>(cmd, args)
}

/** A short opaque idempotency key for a single user action. */
function newIdempotencyKey(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID()
  }
  return `idem-${Date.now()}-${Math.floor(Math.random() * 1e9)}`
}

/**
 * Human-supplied confirm-time refinements. Every field is optional; the server
 * falls back to the candidate's proposal for anything omitted. All values are
 * sanitized, bounded overrides — never raw source content.
 */
export interface ConfirmEdit {
  confirmedTitle?: string | null
  confirmedBody?: string | null
  confirmedDue?: string | null
}

export interface UseDurableTasks {
  candidates: TaskCandidateView[]
  todos: TodoView[]
  loading: boolean
  error: string | null
  refresh: () => Promise<void>
  confirmCandidate: (id: string, revision: number, edit?: ConfirmEdit) => Promise<TaskOutcome>
  dismissCandidate: (id: string, revision: number, reason?: string) => Promise<TaskOutcome>
  transitionTodo: (id: string, target: TodoState, revision: number) => Promise<TaskOutcome>
  deleteTodo: (id: string, revision: number) => Promise<TaskOutcome>
}

export function useDurableTasks(): UseDurableTasks {
  const [candidates, setCandidates] = useState<TaskCandidateView[]>([])
  const [todos, setTodos] = useState<TodoView[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  // Monotonic request id so a slow earlier refresh can never clobber the result
  // of a later one (e.g. the mount refresh landing after a post-generation
  // refresh that already loaded a fresh candidate).
  const requestRef = useRef(0)

  const refresh = useCallback(async () => {
    const request = ++requestRef.current
    setLoading(true)
    try {
      const [nextCandidates, nextTodos] = await Promise.all([
        invoke<TaskCandidateView[]>('list_task_candidates'),
        invoke<TodoView[]>('list_todos'),
      ])
      if (request !== requestRef.current) return
      setCandidates(nextCandidates)
      setTodos(nextTodos)
      setError(null)
    } catch (err) {
      if (request !== requestRef.current) return
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      if (request === requestRef.current) setLoading(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const confirmCandidate = useCallback(
    async (id: string, revision: number, edit?: ConfirmEdit) => {
      const result = await invoke<TaskOutcome>('confirm_task_candidate', {
        candidateId: id,
        expectedRevision: revision,
        idempotencyKey: newIdempotencyKey(),
        confirmedTitle: edit?.confirmedTitle ?? null,
        confirmedBody: edit?.confirmedBody ?? null,
        confirmedDue: edit?.confirmedDue ?? null,
      })
      await refresh()
      return result
    },
    [refresh],
  )

  const dismissCandidate = useCallback(
    async (id: string, revision: number, reason?: string) => {
      const result = await invoke<TaskOutcome>('dismiss_task_candidate', {
        candidateId: id,
        expectedRevision: revision,
        idempotencyKey: newIdempotencyKey(),
        reason: reason ?? null,
      })
      await refresh()
      return result
    },
    [refresh],
  )

  const transitionTodo = useCallback(
    async (id: string, target: TodoState, revision: number) => {
      const result = await invoke<TaskOutcome>('transition_todo', {
        todoId: id,
        targetState: target,
        expectedRevision: revision,
        idempotencyKey: newIdempotencyKey(),
      })
      await refresh()
      return result
    },
    [refresh],
  )

  const deleteTodo = useCallback(
    async (id: string, revision: number) => {
      const result = await invoke<TaskOutcome>('delete_todo', {
        todoId: id,
        expectedRevision: revision,
        idempotencyKey: newIdempotencyKey(),
      })
      await refresh()
      return result
    },
    [refresh],
  )

  return {
    candidates,
    todos,
    loading,
    error,
    refresh,
    confirmCandidate,
    dismissCandidate,
    transitionTodo,
    deleteTodo,
  }
}
