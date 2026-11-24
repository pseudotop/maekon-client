import { fireEvent, screen, within } from '@testing-library/react'
import { Route, Routes } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import type { TaskCandidateView, TodoView } from '../../hooks/useDurableTasks'
import i18n from '../../i18n'
import OverviewNextActions, { NextActionsCard } from './NextActionsCard'
import type { NextActionItem } from './nextActions'

// The desktop branch reads the durable task store over IPC. Both are swapped
// per test: `tauri` picks the branch, `tasks` is what the store answered.
const desktop = vi.hoisted(() => ({
  tauri: false,
  tasks: {
    todos: [] as TodoView[] | null,
    candidates: [] as TaskCandidateView[] | null,
    loading: false,
    error: null as string | null,
  },
}))

vi.mock('../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/platform')>()),
  get IS_TAURI() {
    return desktop.tauri
  },
}))

vi.mock('../../hooks/useDurableTasks', () => ({
  useDurableTasks: () => desktop.tasks,
}))

function renderCard(ui: React.ReactElement) {
  return renderWithProviders(
    <Routes>
      <Route path="/overview" element={ui} />
      <Route path="/tasks" element={<div data-testid="tasks-screen" />} />
    </Routes>,
    { routerProps: { initialEntries: ['/overview'] } },
  )
}

const ITEMS: NextActionItem[] = [
  { kind: 'todo', id: 't1', title: 'Ship the overview', state: 'IN_PROGRESS', due: null },
  { kind: 'todo', id: 't2', title: '   ', state: 'CONFIRMED', due: null },
  { kind: 'candidate', id: 'c1', title: 'Review the sync PR', due: null },
  { kind: 'candidate', id: 'c2', title: 'Reply to design notes', due: null },
]

describe('NextActionsCard', () => {
  it('shows the first three items and opens the Tasks screen from a row', () => {
    renderCard(<NextActionsCard status="ready" items={ITEMS} />)

    const card = screen.getByTestId('next-actions')
    expect(within(card).getByTestId('next-action-todo-t1')).toHaveTextContent('Ship the overview')
    // An item without a usable title still reads as a task, not a blank row.
    expect(within(card).getByTestId('next-action-todo-t2')).toHaveTextContent(i18n.t('durableTasks.noTitle'))
    expect(within(card).getByTestId('next-action-candidate-c1')).toHaveTextContent(
      i18n.t('dashboard.nextActions.suggested'),
    )
    expect(within(card).queryByTestId('next-action-candidate-c2')).not.toBeInTheDocument()
    expect(card).toHaveTextContent(i18n.t('dashboard.nextActions.more', { count: 1 }))

    fireEvent.click(within(card).getByTestId('next-action-todo-t1'))
    expect(screen.getByTestId('tasks-screen')).toBeInTheDocument()
  })

  it('explains how to fill an empty list and links to the Tasks screen', () => {
    renderCard(<NextActionsCard status="ready" items={[]} />)

    const empty = screen.getByTestId('next-actions-empty')
    expect(empty).toHaveTextContent(i18n.t('dashboard.nextActions.emptyDescription'))
    fireEvent.click(within(empty).getByRole('button'))
    expect(screen.getByTestId('tasks-screen')).toBeInTheDocument()
  })

  it('reports a load failure instead of an empty list', () => {
    renderCard(<NextActionsCard status="error" items={[]} />)
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('dashboard.nextActions.loadError'))
    expect(screen.queryByTestId('next-actions-empty')).not.toBeInTheDocument()
  })

  it('marks itself busy while loading', () => {
    renderCard(<NextActionsCard status="loading" items={[]} />)
    expect(screen.getByTestId('next-actions')).toHaveAttribute('aria-busy', 'true')
  })
})

describe('OverviewNextActions outside the desktop app', () => {
  it('says tasks live in the desktop app rather than showing an empty list', () => {
    renderCard(<OverviewNextActions />)
    expect(screen.getByTestId('next-actions-unavailable')).toBeInTheDocument()
    expect(screen.queryByTestId('next-actions-empty')).not.toBeInTheDocument()
  })

  it('renders nothing beside the empty first screen', () => {
    renderCard(<OverviewNextActions hideWhenEmpty />)
    expect(screen.queryByTestId('next-actions')).not.toBeInTheDocument()
  })
})

describe('OverviewNextActions in the desktop app', () => {
  const TODO: TodoView = {
    id: 't1',
    state: 'IN_PROGRESS',
    title: 'Ship the overview',
    body: null,
    due: null,
    revision: 1,
    created_at: '2026-11-10T09:00:00Z',
    updated_at: '2026-11-10T09:00:00Z',
  }

  beforeEach(() => {
    desktop.tauri = true
  })

  afterEach(() => {
    desktop.tauri = false
    desktop.tasks = { todos: [], candidates: [], loading: false, error: null }
  })

  it('lists the open to-dos the task store answered with', () => {
    desktop.tasks = { todos: [TODO], candidates: [], loading: false, error: null }
    renderCard(<OverviewNextActions />)
    expect(screen.getByTestId('next-action-todo-t1')).toHaveTextContent('Ship the overview')
  })

  // The onboarding e2e stubs every unknown IPC command with `null`; before this
  // guard, `null.filter` threw and the error boundary replaced the first screen.
  it('reports a malformed task-store answer as a load failure instead of crashing', () => {
    desktop.tasks = { todos: null, candidates: null, loading: false, error: null }
    renderCard(<OverviewNextActions />)
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('dashboard.nextActions.loadError'))
    expect(screen.queryByTestId('next-actions-empty')).not.toBeInTheDocument()
  })

  it('stays off the empty first screen when the answer is malformed', () => {
    desktop.tasks = { todos: [TODO], candidates: null, loading: false, error: null }
    renderCard(<OverviewNextActions hideWhenEmpty />)
    expect(screen.queryByTestId('next-actions')).not.toBeInTheDocument()
  })
})
