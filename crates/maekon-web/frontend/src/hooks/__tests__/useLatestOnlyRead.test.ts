/**
 * #9492 L2 — the unmount half of the `auth_status` read guard.
 *
 * This suite exists because the assertion it replaces could not fail. React 18
 * drops a `setState` aimed at a detached fiber without a warning, so
 * "render, unmount, resolve the pending read, expect no console error" is green
 * with or without the guard.
 *
 * `isCurrent` is the actual decision the guard makes, so asserting on it is
 * both observable and mutation-provable: deleting the effect cleanup in
 * `useLatestOnlyRead` turns the unmount case RED.
 */

import { renderHook } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { useLatestOnlyRead } from '../useLatestOnlyRead'

describe('useLatestOnlyRead', () => {
  it('treats the most recent token as current', () => {
    const { result } = renderHook(() => useLatestOnlyRead())

    const token = result.current.begin()

    expect(result.current.isCurrent(token)).toBe(true)
  })

  it('invalidates an earlier read once a newer one starts', () => {
    // The stale-response case: a slow read must not apply its result on top of
    // a newer one that already landed.
    const { result } = renderHook(() => useLatestOnlyRead())

    const first = result.current.begin()
    const second = result.current.begin()

    expect(result.current.isCurrent(first)).toBe(false)
    expect(result.current.isCurrent(second)).toBe(true)
  })

  it('invalidates an in-flight read when the owner unmounts', () => {
    // The regression this file exists for. `result.current` keeps the closure
    // over the ref after unmount, so the guard's answer is still readable —
    // unlike the component-level symptom, which React makes invisible.
    const { result, unmount } = renderHook(() => useLatestOnlyRead())

    const inFlight = result.current.begin()
    expect(result.current.isCurrent(inFlight)).toBe(true)

    unmount()

    expect(
      result.current.isCurrent(inFlight),
      'a read still in flight at unmount must not be applied — delete the effect cleanup in useLatestOnlyRead and this assertion is what fails',
    ).toBe(false)
  })

  it('keeps a stable identity across re-renders', () => {
    // Consumers list this object in `useCallback` deps; a fresh object each
    // render would re-arm their effects every render.
    const { result, rerender } = renderHook(() => useLatestOnlyRead())

    const first = result.current
    rerender()

    expect(result.current).toBe(first)
  })
})
