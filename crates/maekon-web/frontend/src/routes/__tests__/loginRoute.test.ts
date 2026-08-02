/**
 * #9603 WD-02.1 — `/login` route registration.
 *
 * The sign-in screen is a route rather than a modal so demo beat 1 can open on
 * it directly. Three properties of that registration are load-bearing and none
 * of them is visible from the page component itself:
 *
 * 1. It carries an `icon` — `CommandPalette::buildNavigationItems` skips
 *    iconless nodes, so without one the route would be reachable only by a
 *    hand-typed deep link, i.e. dead to every real user.
 * 2. It carries neither `group` nor `bottom` — the 48px ActivityBar rail is
 *    permanent chrome, and signing in is optional in a local-first product
 *    (and impossible in the default build, which does not compile connected
 *    mode in at all).
 * 3. It has no `children` — `RouteRenderer`'s dev-time validator throws for a
 *    node with children and no `defaultChild`, and `App.tsx` sizes the sidebar
 *    column from `children.length`.
 */

import { describe, expect, it } from 'vitest'
import en from '../../i18n/locales/en.json'
import { routeTree } from '../route-tree'
import { matchesRoute } from '../useCurrentRoute'

describe('login route registration (#9603 WD-02.1)', () => {
  const login = routeTree.find((route) => route.path === '/login')

  it('registers /login in the single-source routeTree', () => {
    expect(login).toBeDefined()
    expect(login?.labelKey).toBe('nav.signIn')
    expect(en.nav.signIn).toBeTruthy()
  })

  it('carries an icon so the command palette lists it', () => {
    // buildNavigationItems does `if (!node.icon) continue` — an iconless node
    // is silently absent from the palette.
    expect(login?.icon).toBeDefined()
  })

  it('claims no ActivityBar slot', () => {
    expect(login?.group).toBeUndefined()
    expect(login?.bottom).toBeUndefined()
  })

  it('is a childless leaf', () => {
    expect(login?.children).toBeUndefined()
    expect(login?.defaultChild).toBeUndefined()
  })

  it('does not shadow, and is not shadowed by, another top-level route', () => {
    // `matchesRoute` is prefix-based, so a sibling whose path is a prefix of
    // "/login" would swallow it in useCurrentRoute / ActivityBar highlighting.
    const matching = routeTree.filter((route) => matchesRoute(route, '/login'))
    expect(matching.map((route) => route.path)).toEqual(['/login'])
  })
})
