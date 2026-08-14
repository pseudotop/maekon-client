import { describe, expect, it } from 'vitest'
import { routeTree } from '../route-tree'
import { matchesRoute } from '../useCurrentRoute'

describe('assignment email draft route (#9627 WD-04.4)', () => {
  const route = routeTree.find((candidate) => candidate.path === '/assignment-email-draft')

  it('is registered with an icon for command-palette discovery', () => {
    expect(route?.labelKey).toBe('nav.assignmentEmailDraft')
    expect(route?.icon).toBeDefined()
  })

  it('does not claim permanent navigation chrome', () => {
    expect(route?.group).toBeUndefined()
    expect(route?.bottom).toBeUndefined()
    expect(route?.children).toBeUndefined()
  })

  it('does not collide with another top-level route', () => {
    const matching = routeTree.filter((candidate) => matchesRoute(candidate, '/assignment-email-draft'))
    expect(matching.map((candidate) => candidate.path)).toEqual(['/assignment-email-draft'])
  })
})
