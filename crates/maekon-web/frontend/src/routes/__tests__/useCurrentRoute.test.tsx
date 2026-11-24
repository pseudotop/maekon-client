import { renderHook } from '@testing-library/react'
import type { ReactNode } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it } from 'vitest'
import { useCurrentGroup, useCurrentRoute } from '../useCurrentRoute'

function wrapperForPath(path: string) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <MemoryRouter initialEntries={[path]}>{children}</MemoryRouter>
  }
}

describe('useCurrentRoute', () => {
  it('resolves root "/" to the root route node (no child when at default)', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/') })
    expect(result.current.node.path).toBe('/')
    // At the bare "/" path, before redirect to defaultChild, child is null
    expect(result.current.child).toBeNull()
  })

  it('resolves "/overview" to the root route with overview as active child', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/overview') })
    expect(result.current.node.path).toBe('/')
    expect(result.current.child?.path).toBe('overview')
  })

  it('resolves "/monitoring" to its own leaf, not a Dashboard child (#13735)', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/monitoring') })
    expect(result.current.node.path).toBe('/monitoring')
    expect(result.current.child).toBeNull()
  })

  it('resolves "/focus/score" to /focus with score as active child', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/focus/score') })
    expect(result.current.node.path).toBe('/focus')
    expect(result.current.child?.path).toBe('score')
  })

  it('resolves "/focus/sessions" to /focus with sessions as active child', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/focus/sessions') })
    expect(result.current.node.path).toBe('/focus')
    expect(result.current.child?.path).toBe('sessions')
  })

  it('resolves "/automation/policies" to /automation with policies as active child', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/automation/policies') })
    expect(result.current.node.path).toBe('/automation')
    expect(result.current.child?.path).toBe('policies')
  })

  it('resolves "/settings/ai-automation" to /settings with ai-automation as active child', () => {
    const { result } = renderHook(() => useCurrentRoute(), {
      wrapper: wrapperForPath('/settings/ai-automation'),
    })
    expect(result.current.node.path).toBe('/settings')
    expect(result.current.child?.path).toBe('ai-automation')
  })

  it('resolves a childless leaf route "/day" with no active child', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/day') })
    expect(result.current.node.path).toBe('/day')
    expect(result.current.child).toBeNull()
  })

  it('resolves a childless leaf route "/week" with no active child (#5676)', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/week') })
    expect(result.current.node.path).toBe('/week')
    expect(result.current.child).toBeNull()
  })

  it('resolves a childless leaf route "/chat" with no active child', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/chat') })
    expect(result.current.node.path).toBe('/chat')
    expect(result.current.child).toBeNull()
  })

  it('resolves an unknown path to the root route as fallback', () => {
    const { result } = renderHook(() => useCurrentRoute(), { wrapper: wrapperForPath('/nonexistent-route') })
    expect(result.current.node.path).toBe('/')
    expect(result.current.child).toBeNull()
  })
})

describe('useCurrentGroup', () => {
  const cases: Array<[path: string, expected: string | null]> = [
    // #13736: one `main` group holds the front destinations and the bundles
    // one level back, so every former Monitor / Insights / Manage route
    // resolves to it.
    ['/', 'main'],
    ['/overview', 'main'],
    ['/day', 'main'],
    ['/week', 'main'],
    ['/timeline/all', 'main'],
    ['/replay/timeline', 'main'],
    ['/focus/score', 'main'],
    ['/reports/activity', 'main'],
    ['/coaching/goals', 'main'],
    ['/chat', 'main'],
    ['/playbooks', 'main'],
    ['/search', 'main'],
    ['/automation/policies', 'main'],
    ['/recalibration/segments', 'main'],
    ['/audit/summary', 'main'],
    ['/policies', 'main'],
    ['/tasks', 'main'],
    ['/updates/status', 'main'],
    // Support moved from a bottom icon into the System bundle (#13736)
    ['/support', 'main'],
    // System Metrics left the Dashboard (#13735) for the System bundle
    ['/monitoring', 'main'],
    // Bottom routes — not in any group
    ['/settings/general', null],
    ['/settings/monitoring', null],
    ['/privacy/data', null],
    // Palette-only routes — no group, no rail slot
    ['/login', null],
    // Fallback — unknown path falls back to "/" which is in the main group
    ['/nonexistent', 'main'],
  ]

  for (const [path, expected] of cases) {
    it(`returns ${expected ?? 'null'} for ${path}`, () => {
      const { result } = renderHook(() => useCurrentGroup(), { wrapper: wrapperForPath(path) })
      expect(result.current).toBe(expected)
    })
  }
})
