import { describe, expect, it } from 'vitest'
import ClaimsSection from '../../pages/privacy-page/ClaimsSection'
import DataSection from '../../pages/privacy-page/DataSection'
import EgressLedgerSection from '../../pages/privacy-page/EgressLedgerSection'
import { routeTree } from '../route-tree'

const privacyRoute = routeTree.find((route) => route.path === '/privacy')
const childByPath = (path: string) => privacyRoute?.children?.find((child) => child.path === path)

// React.lazy stores the import factory at `_payload._result` while the lazy
// component is still Uninitialized (never rendered). Invoke it to load the real
// module so we can assert exact route→component identity. A distinctness test
// below guards the same regression without touching internals, so a future
// React change degrades to a weaker (but still failing-on-regression) check
// rather than a false green.
type LazyInternals = { _payload?: { _result?: unknown } }

async function resolveLazyComponent(component: unknown): Promise<unknown> {
  const factory = (component as LazyInternals)._payload?._result
  if (typeof factory !== 'function') {
    throw new Error('route component is not an unresolved React.lazy factory')
  }
  const mod = (await (factory as () => Promise<{ default?: unknown }>)()) as { default?: unknown }
  return mod.default ?? mod
}

describe('privacy routes', () => {
  it('labels destructive data deletion as a danger zone, not consent', () => {
    const destructiveChild = childByPath('consent')
    expect(destructiveChild?.labelKey).toBe('sidebar.dangerZone')
  })

  // #8095 (CJ-00-08 / CJ-04-03..06): the egress-ledger and memory-claims leaves
  // reportedly both rendered the Data-Controls body. On origin/main they route
  // to fully-distinct components; pin that mapping so a regression (either leaf
  // aliasing DataSection, or egress/claims being swapped) fails the build.
  it('routes data, egress, and claims to three distinct components', () => {
    const data = childByPath('data')?.component
    const egress = childByPath('egress')?.component
    const claims = childByPath('claims')?.component

    expect(data).toBeDefined()
    expect(egress).toBeDefined()
    expect(claims).toBeDefined()
    expect(egress).not.toBe(data)
    expect(claims).not.toBe(data)
    expect(egress).not.toBe(claims)
  })

  it('pins egress → EgressLedgerSection and claims → ClaimsSection', async () => {
    expect(await resolveLazyComponent(childByPath('egress')?.component)).toBe(EgressLedgerSection)
    expect(await resolveLazyComponent(childByPath('claims')?.component)).toBe(ClaimsSection)
    // Anchor the reference the regression aliased everything to.
    expect(await resolveLazyComponent(childByPath('data')?.component)).toBe(DataSection)
  })
})
