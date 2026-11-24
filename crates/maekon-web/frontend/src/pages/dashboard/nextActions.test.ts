import { describe, expect, it } from 'vitest'
import type { TaskCandidateView, TodoView } from '../../hooks/useDurableTasks'
import { buildNextActions } from './nextActions'

const NOW = Date.parse('2026-11-10T09:00:00Z')

function todo(id: string, state: TodoView['state'], due: string | null = null): TodoView {
  return {
    id,
    state,
    title: `Todo ${id}`,
    body: null,
    due,
    revision: 1,
    created_at: '2026-11-09T09:00:00Z',
    updated_at: '2026-11-09T09:00:00Z',
  }
}

function candidate(id: string, state: TaskCandidateView['state'], expiresAt: string): TaskCandidateView {
  return {
    id,
    state,
    title: `Candidate ${id}`,
    body: null,
    proposed_due: null,
    expires_at: expiresAt,
    source_kind: 'next_step',
    source_lifecycle: 'active',
    revision: 1,
  }
}

describe('buildNextActions', () => {
  it('lists work in motion first, then blocked work, then fresh to-dos, each by due date', () => {
    const items = buildNextActions(
      [
        todo('confirmed-late', 'CONFIRMED', '2026-11-20T00:00:00Z'),
        todo('waiting', 'WAITING'),
        todo('confirmed-soon', 'CONFIRMED', '2026-11-11T00:00:00Z'),
        todo('in-progress', 'IN_PROGRESS'),
        todo('confirmed-undated', 'CONFIRMED'),
      ],
      [],
      NOW,
    )
    expect(items.map((item) => item.id)).toEqual([
      'in-progress',
      'waiting',
      'confirmed-soon',
      'confirmed-late',
      'confirmed-undated',
    ])
  })

  it('leaves finished and cancelled to-dos out', () => {
    const items = buildNextActions([todo('done', 'DONE'), todo('cancelled', 'CANCELLED')], [], NOW)
    expect(items).toEqual([])
  })

  it('adds suggestions still waiting for review after the to-dos, in server order', () => {
    const items = buildNextActions(
      [todo('a', 'CONFIRMED')],
      [
        candidate('fresh', 'PROPOSED', '2026-11-10T18:00:00Z'),
        candidate('expired', 'PROPOSED', '2026-11-10T08:00:00Z'),
        candidate('confirmed', 'CONFIRMED', '2026-11-10T18:00:00Z'),
        candidate('unparsable-expiry', 'PROPOSED', 'not-a-date'),
      ],
      NOW,
    )
    expect(items.map((item) => `${item.kind}:${item.id}`)).toEqual([
      'todo:a',
      'candidate:fresh',
      'candidate:unparsable-expiry',
    ])
  })

  it('is empty when there is nothing to pick up', () => {
    expect(buildNextActions([], [], NOW)).toEqual([])
  })
})
