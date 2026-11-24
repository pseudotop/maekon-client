import { screen, waitFor } from '@testing-library/react'
import { useLocation } from 'react-router-dom'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import RouteRenderer from '../RouteRenderer'

// #13736 moved 14 destinations one level back in the side panel without
// changing a single path. Notification clicks, tray and shortcut deep links,
// command-palette entries and bookmarked URLs all depend on that, so this
// list freezes every URL the route tree served at main 209c53662d (24
// top-level routes, 41 child routes) together with where it lands.
//
// Each case renders the real RouteRenderer — its sorting, `/*` splats,
// index redirects to `defaultChild` and the catch-all — with every page
// swapped for a stub that names the route entry it belongs to. A URL passes
// only when it lands on its expected path AND that path's own entry renders;
// falling through to the catch-all redirect to "/" fails both checks.
vi.mock('../route-tree', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../route-tree')>()
  const { createElement } = await import('react')
  const { Outlet } = await import('react-router-dom')
  const stub = (screenPath: string, layout: boolean) => {
    function RouteStub() {
      return createElement('section', { 'data-screen': screenPath }, layout ? createElement(Outlet) : null)
    }
    return RouteStub
  }
  return {
    ...actual,
    routeTree: actual.routeTree.map((node) => ({
      ...node,
      component: stub(node.path, Boolean(node.children)),
      children: node.children?.map((child) => ({
        ...child,
        component: stub(actual.joinChildPath(node, child), false),
      })),
    })),
  }
})

/** [URL served before #13736, path it must land on] */
const LEGACY_URLS: ReadonlyArray<readonly [url: string, landsOn: string]> = [
  ['/', '/overview'],
  ['/overview', '/overview'],
  ['/monitoring', '/monitoring'],
  ['/insights', '/insights'],
  ['/day', '/day'],
  ['/week', '/week'],
  ['/timeline', '/timeline/all'],
  ['/timeline/all', '/timeline/all'],
  ['/timeline/filters', '/timeline/filters'],
  ['/replay', '/replay/timeline'],
  ['/replay/timeline', '/replay/timeline'],
  ['/replay/events', '/replay/events'],
  ['/focus', '/focus/score'],
  ['/focus/score', '/focus/score'],
  ['/focus/sessions', '/focus/sessions'],
  ['/focus/interruptions', '/focus/interruptions'],
  ['/reports', '/reports/activity'],
  ['/reports/activity', '/reports/activity'],
  ['/reports/focus', '/reports/focus'],
  ['/reports/export', '/reports/export'],
  ['/coaching', '/coaching/goals'],
  ['/coaching/goals', '/coaching/goals'],
  ['/coaching/history', '/coaching/history'],
  ['/chat', '/chat'],
  ['/playbooks', '/playbooks'],
  ['/search', '/search'],
  ['/automation', '/automation/policies'],
  ['/automation/policies', '/automation/policies'],
  ['/automation/commands', '/automation/commands'],
  ['/automation/history', '/automation/history'],
  ['/recalibration', '/recalibration/segments'],
  ['/recalibration/segments', '/recalibration/segments'],
  ['/recalibration/overrides', '/recalibration/overrides'],
  ['/policies', '/policies'],
  ['/tasks', '/tasks'],
  ['/audit', '/audit/summary'],
  ['/audit/summary', '/audit/summary'],
  ['/audit/entries', '/audit/entries'],
  ['/updates', '/updates/status'],
  ['/updates/status', '/updates/status'],
  ['/updates/channel', '/updates/channel'],
  ['/settings', '/settings/general'],
  ['/settings/general', '/settings/general'],
  ['/settings/privacy', '/settings/privacy'],
  ['/settings/monitoring', '/settings/monitoring'],
  ['/settings/coaching', '/settings/coaching'],
  ['/settings/audio', '/settings/audio'],
  ['/settings/ai-automation', '/settings/ai-automation'],
  ['/settings/data', '/settings/data'],
  ['/settings/sync', '/settings/sync'],
  ['/settings/integrations', '/settings/integrations'],
  ['/settings/focus-auto', '/settings/focus-auto'],
  ['/settings/advanced', '/settings/advanced'],
  ['/settings/tracking-schedule', '/settings/tracking-schedule'],
  ['/privacy', '/privacy/data'],
  ['/privacy/data', '/privacy/data'],
  ['/privacy/egress', '/privacy/egress'],
  ['/privacy/claims', '/privacy/claims'],
  ['/privacy/consent', '/privacy/consent'],
  ['/privacy/export', '/privacy/export'],
  ['/support', '/support'],
  ['/login', '/login'],
  ['/home', '/home'],
  ['/assignment-email-draft', '/assignment-email-draft'],
  ['/wbs-xlsx-draft', '/wbs-xlsx-draft'],
]

function LocationProbe() {
  return <output data-testid="location">{useLocation().pathname}</output>
}

describe('legacy URLs keep opening the same screen (#13736)', () => {
  it('covers the whole pre-#13736 route tree', () => {
    expect(LEGACY_URLS).toHaveLength(65)
    expect(new Set(LEGACY_URLS.map(([url]) => url)).size).toBe(LEGACY_URLS.length)
  })

  for (const [url, landsOn] of LEGACY_URLS) {
    it(`${url} → ${landsOn}`, async () => {
      renderWithProviders(
        <>
          <RouteRenderer />
          <LocationProbe />
        </>,
        { routerProps: { initialEntries: [url] } },
      )

      await waitFor(() => expect(screen.getByTestId('location').textContent).toBe(landsOn))
      // The deepest rendered stub is the entry registered at that exact path,
      // so the page shown is the one the URL named before.
      const screens = Array.from(document.querySelectorAll<HTMLElement>('[data-screen]'))
      expect(screens.at(-1)?.dataset.screen).toBe(landsOn)
    })
  }
})
