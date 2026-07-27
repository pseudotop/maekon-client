import { useSyncExternalStore } from 'react'

/**
 * #8686 AC4: OS screen-capture permission status pushed from the Rust monitor
 * loop via the `capture-os-permission` Tauri event (registered in
 * `useTauriEventBridge`). While `blocked` is true the app renders a persistent
 * recovery banner (`CapturePermissionNotice`) — capture is already stopped
 * fail-closed on the Rust side; this store only drives the user-facing
 * guidance.
 *
 * Module-scoped store (same pattern as `useToast`) so the event bridge can
 * update it without a provider, and multiple consumers stay in sync.
 */
interface OsPermissionStore {
  blocked: boolean
  listeners: Set<() => void>
}

const store: OsPermissionStore = {
  blocked: false,
  listeners: new Set(),
}

function notify() {
  store.listeners.forEach((listener) => {
    listener()
  })
}

function subscribe(listener: () => void) {
  store.listeners.add(listener)
  return () => store.listeners.delete(listener)
}

function getSnapshot() {
  return store.blocked
}

/** Sets the blocked state; no-op (no re-render) when the value is unchanged. */
export function setOsCapturePermissionBlocked(blocked: boolean) {
  if (store.blocked === blocked) return
  store.blocked = blocked
  notify()
}

/** Test-only helper: reset the module store between test cases. */
export function resetOsPermissionStatusForTest() {
  store.blocked = false
  notify()
}

/** Whether the OS screen-capture permission is currently revoked. */
export function useOsCapturePermissionBlocked(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}
