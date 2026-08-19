import { describe, expect, it } from 'vitest'
import en from '../../i18n/locales/en.json'
import { routeTree } from '../route-tree'

// #8079 / #8080: the Support destination and the Integrations settings tab must
// be registered in the single-source routeTree so navigation, ActivityBar, and
// SettingsLayout tab derivation all pick them up.
describe('support + integrations route registration', () => {
  it('registers a discoverable bottom-level /support destination', () => {
    const support = routeTree.find((route) => route.path === '/support')
    expect(support).toBeDefined()
    expect(support?.bottom).toBe(true)
    expect(support?.icon).toBeDefined()
    expect(support?.labelKey).toBe('nav.support')
    // The nav label must resolve in the baseline locale.
    expect(en.nav.support).toBeTruthy()
  })

  it('registers the Integrations tab as a Settings child in the Advanced group', () => {
    const settings = routeTree.find((route) => route.path === '/settings')
    const integrations = settings?.children?.find((child) => child.path === 'integrations')
    expect(integrations).toBeDefined()
    expect(integrations?.labelKey).toBe('settings.tabs.integrations')

    // Must be grouped so the SidePanel (grouped bottom-mode) renders it — an
    // orphan child would show in the collapsed Tabs but vanish from the tree.
    const advanced = settings?.childGroups?.find((group) => group.labelKey === 'settings.groupAdvanced')
    expect(advanced?.tabs).toContain('integrations')
    expect(en.settings.tabs.integrations).toBeTruthy()
  })
})
