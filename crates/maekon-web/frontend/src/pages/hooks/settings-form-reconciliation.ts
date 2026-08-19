/**
 * Reconcile a refreshed settings payload without overwriting a real user edit.
 *
 * `previousSerialized` must be captured before scheduling a React state
 * updater. Reading a mutable ref inside the updater races with the caller
 * advancing that ref to the refreshed payload and can create a stale dirty
 * state after a successful save.
 */
export function reconcileLoadedSettings<T>(current: T | null, previousSerialized: string | null, incoming: T): T {
  if (!current) return incoming
  if (previousSerialized && JSON.stringify(current) === previousSerialized) return incoming
  return current
}
