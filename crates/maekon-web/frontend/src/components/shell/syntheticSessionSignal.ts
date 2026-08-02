/**
 * Latched "this session is showing synthetic demo data" signal (#9611 WD-02.3).
 *
 * ## Why a latch, and why module-level
 *
 * The requirement is that the synthetic-data label lives in the **app shell**
 * and does not disappear during route, modal, or error transitions. The
 * evidence for the label, though, arrives on the context-home snapshot
 * (`synthetic` / `provenance.synthetic_only`, #9625) — which is fetched by one
 * page, and which is *absent* exactly when the home is unavailable or the user
 * has navigated away.
 *
 * Deriving the badge from the live snapshot would therefore make it blink out
 * in precisely the situations the requirement names. Latching is what closes
 * that: once a session has produced positive evidence that it is serving
 * synthetic data, the shell keeps saying so until the session ends.
 *
 * Module-level rather than React context because the producer (the home page,
 * deep in the route tree) and the consumer (the shell, above the router) have
 * no common ancestor below `App`. Threading a provider through `App` for one
 * boolean would put demo-labelling machinery in the shell's constructor for
 * every build, including the ones that have no connected mode at all.
 *
 * ## Why it is not simply "always on when signed in"
 *
 * A badge that asserts "synthetic demo data" purely from being authenticated
 * would be a false claim the moment anyone connects this client to a real
 * server. The label is a statement about the data, so it is made only on
 * evidence from the data. Before any evidence, the shell says nothing — which
 * is honest, and is not one of the transitions the requirement protects.
 *
 * ## Lifecycle
 *
 * Set on the first snapshot that reports synthetic provenance. Cleared only by
 * [`clearSyntheticSession`], which sign-out calls — a new session must re-earn
 * the claim rather than inherit the previous one.
 */

export interface SyntheticSessionEvidence {
  /** The server said this snapshot is synthetic. */
  synthetic: boolean
  /** Seed namespaces the server attributed the rows to, if any. */
  seedNamespaces: readonly string[]
}

let latched: SyntheticSessionEvidence | null = null
const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

/**
 * Record evidence from a snapshot.
 *
 * Only positive evidence latches. A later snapshot that omits the flag does
 * **not** clear it: an unavailable or partially-served home is not evidence
 * that the session stopped being a demo, and treating it as such is the flicker
 * this module exists to prevent.
 */
export function markSyntheticSession(evidence: SyntheticSessionEvidence): void {
  if (!evidence.synthetic) return
  const sameNamespaces =
    latched !== null &&
    latched.seedNamespaces.length === evidence.seedNamespaces.length &&
    latched.seedNamespaces.every((ns, i) => ns === evidence.seedNamespaces[i])
  if (latched !== null && sameNamespaces) return

  latched = { synthetic: true, seedNamespaces: [...evidence.seedNamespaces] }
  emit()
}

/** Drop the latch. Sign-out calls this so the next session re-earns the claim. */
export function clearSyntheticSession(): void {
  if (latched === null) return
  latched = null
  emit()
}

/** Current evidence, or `null` if this session has produced none. */
export function readSyntheticSession(): SyntheticSessionEvidence | null {
  return latched
}

/** Subscribe to latch changes. Returns an unsubscribe function. */
export function subscribeSyntheticSession(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/**
 * Test-only reset.
 *
 * Module-level state outlives a test's render tree, so without this a test that
 * latches the badge leaks into every test that runs after it — the kind of
 * order-dependent green that hides a real regression.
 */
export function __resetSyntheticSessionForTests(): void {
  latched = null
  listeners.clear()
}
