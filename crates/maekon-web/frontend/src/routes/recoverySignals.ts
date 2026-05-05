/**
 * Recovery signal registry — internal pub/sub between useTauriEventBridge
 * and RouteErrorBoundary instances.
 *
 * This module replaces the prior `window.dispatchEvent('route-error-reset')`
 * approach. The CustomEvent on window was spoofable by any script with global
 * access (XSS, browser extensions, third-party libraries). A module-level
 * typed registry has these properties:
 *
 *  - **Not exposed to globals** — only modules that import this file can
 *    subscribe or notify.
 *  - **Type-safe** — listeners are `() => void`, no payload coupling.
 *  - **Per-route routing** — subscribers register for a specific route, only
 *    matching notifications fire.
 *  - **Cleanup-safe** — subscribe returns an unsubscribe function.
 *
 * Used by:
 *  - `useTauriEventBridge.ts` — calls `notifyRouteRecovery(route)` when Rust
 *    sends a `frontend-recovery` event with strategy `reset-route`.
 *  - `RouteErrorBoundary.tsx` — subscribes per route, increments resetKey
 *    when its route fires.
 */

type RecoveryListener = () => void

const listeners = new Map<string, Set<RecoveryListener>>()

/**
 * Subscribe to recovery signals for a specific route.
 * Returns an unsubscribe function — call it on unmount/cleanup.
 */
export function subscribeToRouteRecovery(route: string, listener: RecoveryListener): () => void {
  let set = listeners.get(route)
  if (!set) {
    set = new Set()
    listeners.set(route, set)
  }
  set.add(listener)

  return () => {
    const current = listeners.get(route)
    if (!current) return
    current.delete(listener)
    if (current.size === 0) {
      listeners.delete(route)
    }
  }
}

/**
 * Notify all subscribers of a recovery signal for the given route.
 * No-op if no subscribers exist.
 */
export function notifyRouteRecovery(route: string): void {
  const set = listeners.get(route)
  if (!set) return
  // Snapshot the listener set before iterating in case a listener
  // unsubscribes itself during the call.
  for (const listener of [...set]) {
    listener()
  }
}

/**
 * Test helper — clear all listeners. Only intended for use in unit tests.
 */
export function _resetRecoverySignalsForTest(): void {
  listeners.clear()
}
