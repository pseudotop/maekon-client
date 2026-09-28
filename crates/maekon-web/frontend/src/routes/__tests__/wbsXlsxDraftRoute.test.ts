import { describe, expect, it } from 'vitest'
import { routeTree } from '../route-tree'
import { matchesRoute } from '../useCurrentRoute'

describe('WBS XLSX draft route (#11120 WD-04.5)', () => {
  const route = routeTree.find((candidate) => candidate.path === '/wbs-xlsx-draft')

  it('is command-palette discoverable without permanent rail ownership', () => {
    expect(route?.labelKey).toBe('nav.wbsXlsxDraft')
    expect(route?.icon).toBeDefined()
    expect(route?.group).toBeUndefined()
    expect(route?.bottom).toBeUndefined()
  })

  it('does not collide with another top-level route', () => {
    const matching = routeTree.filter((candidate) => matchesRoute(candidate, '/wbs-xlsx-draft'))
    expect(matching.map((candidate) => candidate.path)).toEqual(['/wbs-xlsx-draft'])
  })
})
