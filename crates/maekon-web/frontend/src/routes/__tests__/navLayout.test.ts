import { describe, expect, it } from 'vitest'
import { getGroupLayout, navGroups, routeTree } from '../route-tree'

// #13736: a newcomer used to face 20 destinations (17 in the three rail
// groups' trees plus three bottom icons). The budget is 6–8 visible up front;
// everything else stays reachable one level back.
const FRONT_DESTINATION_LIMIT = 8

// The destinations the rail and its trees offered before #13736 (main 209c53662d).
const FORMER_RAIL_DESTINATIONS = [
  '/',
  '/day',
  '/week',
  '/timeline',
  '/replay',
  '/focus',
  '/reports',
  '/coaching',
  '/chat',
  '/playbooks',
  '/search',
  '/automation',
  '/recalibration',
  '/policies',
  '/tasks',
  '/audit',
  '/updates',
  '/settings',
  '/privacy',
  '/support',
]

describe('main navigation layout (#13736)', () => {
  const layout = getGroupLayout('main')
  const bottom = routeTree.filter((route) => route.bottom && route.icon)

  it('has a single rail group that lands on the dashboard', () => {
    expect(navGroups.map((group) => [group.id, group.defaultPath])).toEqual([['main', '/']])
  })

  it('keeps six daily destinations at the first side-panel level', () => {
    expect(layout.front.map((route) => route.path)).toEqual([
      '/',
      '/timeline',
      '/search',
      '/focus',
      '/reports',
      '/chat',
    ])
  })

  it('shows at most eight destinations up front, counting the bottom rail icons', () => {
    expect(bottom.map((route) => route.path)).toEqual(['/settings', '/privacy'])
    expect(layout.front.length).toBeLessThanOrEqual(FRONT_DESTINATION_LIMIT)
    expect(layout.front.length + bottom.length).toBeLessThanOrEqual(FRONT_DESTINATION_LIMIT)
  })

  it('moves every other group route one level back into a bundle', () => {
    expect(layout.sections.map(({ section, routes }) => [section.id, routes.map((route) => route.path)])).toEqual([
      ['more', ['/day', '/week', '/replay', '/coaching', '/tasks']],
      ['tools', ['/automation', '/playbooks', '/policies', '/recalibration', '/audit']],
      ['system', ['/updates', '/support', '/monitoring']],
    ])
  })

  it('keeps every former rail destination reachable from the rail or the tree', () => {
    const reachable = new Set(
      [...layout.front, ...layout.sections.flatMap(({ routes }) => routes), ...bottom].map((route) => route.path),
    )
    expect(FORMER_RAIL_DESTINATIONS.filter((path) => !reachable.has(path))).toEqual([])
  })

  it('never files a route under a bundle outside its group', () => {
    for (const route of routeTree) {
      if (route.section) expect(route.group).toBe('main')
    }
  })

  it('gives every navigation destination an icon, so the command palette lists it', () => {
    expect(routeTree.filter((route) => (route.group || route.bottom) && !route.icon)).toEqual([])
  })
})
