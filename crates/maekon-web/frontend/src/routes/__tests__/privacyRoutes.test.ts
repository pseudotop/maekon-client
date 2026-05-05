import { describe, expect, it } from 'vitest'
import { routeTree } from '../route-tree'

describe('privacy routes', () => {
  it('labels destructive data deletion as a danger zone, not consent', () => {
    const privacyRoute = routeTree.find((route) => route.path === '/privacy')
    const destructiveChild = privacyRoute?.children?.find((child) => child.path === 'consent')

    expect(destructiveChild?.labelKey).toBe('sidebar.dangerZone')
  })
})
