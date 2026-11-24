/**
 * "Next up" model for the first screen (#13735).
 *
 * Reads the durable task data that already exists (ADR-028): confirmed to-dos
 * the user is still working on, then suggested tasks waiting for review. Both
 * live on the `/tasks` route, which is where an item opens. Building this list
 * never confirms, dismisses, or transitions anything.
 */

import type { TaskCandidateView, TodoState, TodoView } from '../../hooks/useDurableTasks'

export const NEXT_ACTIONS_PATH = '/tasks'

export type OpenTodoState = Extract<TodoState, 'IN_PROGRESS' | 'WAITING' | 'CONFIRMED'>

export type NextActionItem =
  | { kind: 'todo'; id: string; title: string; state: OpenTodoState; due: string | null }
  | { kind: 'candidate'; id: string; title: string | null; due: string | null }

// Work already in motion first, then work that is blocked, then fresh to-dos.
const OPEN_TODO_ORDER: Record<OpenTodoState, number> = { IN_PROGRESS: 0, WAITING: 1, CONFIRMED: 2 }

function isOpenTodo(todo: TodoView): todo is TodoView & { state: OpenTodoState } {
  return todo.state in OPEN_TODO_ORDER
}

function dueRank(due: string | null): number {
  if (!due) return Number.POSITIVE_INFINITY
  const parsed = Date.parse(due)
  return Number.isNaN(parsed) ? Number.POSITIVE_INFINITY : parsed
}

export function buildNextActions(
  todos: readonly TodoView[],
  candidates: readonly TaskCandidateView[],
  now: number = Date.now(),
): NextActionItem[] {
  const openTodos = todos
    .filter(isOpenTodo)
    .sort((a, b) => OPEN_TODO_ORDER[a.state] - OPEN_TODO_ORDER[b.state] || dueRank(a.due) - dueRank(b.due))
    .map<NextActionItem>((todo) => ({ kind: 'todo', id: todo.id, title: todo.title, state: todo.state, due: todo.due }))

  const reviewable = candidates
    .filter((candidate) => {
      if (candidate.state !== 'PROPOSED') return false
      const expiresAt = Date.parse(candidate.expires_at)
      return Number.isNaN(expiresAt) || expiresAt > now
    })
    .map<NextActionItem>((candidate) => ({
      kind: 'candidate',
      id: candidate.id,
      title: candidate.title,
      due: candidate.proposed_due,
    }))

  return [...openTodos, ...reviewable]
}
