import { describe, expect, it } from 'vitest'
import { routeTree } from '../route-tree'

describe('capture-history route re-authentication', () => {
  it('wraps search with the capture re-authentication gate', () => {
    const searchRoute = routeTree.find((route) => route.path === '/search')

    expect(searchRoute).toBeDefined()
    expect((searchRoute?.component as { displayName?: string }).displayName).toContain('withCaptureReauthGate')
  })

  it('wraps the personal-data export leaf with the re-authentication gate', () => {
    const privacyRoute = routeTree.find((route) => route.path === '/privacy')
    const exportLeaf = privacyRoute?.children?.find((route) => route.path === 'export')

    expect(exportLeaf).toBeDefined()
    expect((exportLeaf?.component as { displayName?: string }).displayName).toContain('withCaptureReauthGate')
  })
})
