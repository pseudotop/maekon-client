import { useCallback, useEffect, useMemo, useRef } from 'react'

/**
 * Guard for a query that can be in flight more than once at a time.
 *
 * Callers `begin()` before awaiting and check `isCurrent(token)` before
 * applying the result. A token stops being current as soon as a newer read
 * starts, or as soon as the owning component unmounts.
 */
export interface LatestOnlyRead {
  /** Claim the newest slot. Returns the token to re-check once the read settles. */
  begin: () => number
  /** True only while `token` is the newest read *and* the owner is still mounted. */
  isCurrent: (token: number) => boolean
}

/**
 * Why this is a hook rather than three lines inline (#9492 L2).
 *
 * The unmount half of this guard has no observable effect in the component that
 * uses it: React 18 silently ignores a `setState` aimed at a detached fiber, so
 * a test that renders a component, unmounts it, and resolves a pending read
 * passes identically whether or not the guard exists — the first attempt at
 * this regression test asserted `console.error` was never called and was
 * therefore incapable of failing.
 *
 * Pulling the counter into its own unit makes the decision itself the
 * observable: `isCurrent` is a plain function whose answer flips at unmount,
 * which a test can assert directly and a mutation can make RED.
 * `__tests__/useLatestOnlyRead.test.ts` does exactly that.
 *
 * The stale-read half *is* observable in the component (a slow read must not
 * overwrite a newer one) and is covered there.
 */
export function useLatestOnlyRead(): LatestOnlyRead {
  const seq = useRef(0)

  useEffect(() => {
    // Bumping on unmount invalidates every token handed out so far, which is
    // what the `cancelled` flag in a single-read `useEffect` used to do.
    return () => {
      seq.current += 1
    }
  }, [])

  const begin = useCallback(() => {
    seq.current += 1
    return seq.current
  }, [])

  const isCurrent = useCallback((token: number) => token === seq.current, [])

  // Stable identity: consumers put this in `useCallback` dependency lists, and a
  // fresh object each render would re-arm their effects on every render.
  return useMemo(() => ({ begin, isCurrent }), [begin, isCurrent])
}
