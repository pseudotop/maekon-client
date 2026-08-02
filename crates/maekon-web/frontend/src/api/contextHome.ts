/**
 * Context-home IPC surface (#9625).
 *
 * One call, no arguments. The Rust side resolves *whose* home this is from the
 * JWT it already holds (`src-tauri/src/commands/context_home.rs`), which is why
 * there is no `userId`/`organizationId` parameter here — not an omission, a
 * boundary. A parameter would make "request someone else's home" expressible
 * from the WebView, leaving a server-side check as the only thing between that
 * and a leak.
 *
 * The bearer never reaches this file. It is attached inside `maekon-network`
 * and appears in no argument, return value, or error on this path.
 *
 * These types mirror `crates/maekon-core/src/models/context_home.rs`, which in
 * turn mirrors the server's `context-home.v1` contract. The authoritative
 * artifact is `api/fixtures/context-home.v1.json`: the server generates it from
 * its own DTOs and byte-compares it in CI, and the Rust wire types parse it in
 * their own tests — so a server-side field change turns those red before it can
 * reach this file silently.
 */

import { isIpcError } from './desktop'

/** Contract version this client is written against. */
export const CONTEXT_HOME_CONTRACT_VERSION = 'context-home.v1'

/**
 * Section state.
 *
 * `unavailable` is the reason this is not just an array. The server could not
 * answer for this section while the rest of the snapshot is valid, and drawing
 * that as "you have no mail" is the specific defect the server contract was
 * shaped to prevent.
 *
 * Typed as a union of the known tokens *plus* `string`: an older client must not
 * blank the whole home because the server introduced a status it has not heard
 * of. Narrow with a `switch` and keep a default branch.
 */
export type SectionStatus = 'ready' | 'empty' | 'unavailable' | (string & {})

/** Why a section could not be served. Same open-union reasoning as above. */
export type SectionUnavailableReason = 'timeout' | 'backend_unavailable' | (string & {})

/**
 * Participant kind.
 *
 * Load-bearing: the two ID spaces are disjoint, and an external counterparty
 * contact is **not** a tenant member and not an authenticated actor. Render one
 * as the other and the UI is making a claim about a real person that is false.
 */
export type ParticipantKind = 'internal_member' | 'external_counterparty_contact' | (string & {})

export interface ContextHomeParticipant {
  participant_id: string
  kind: ParticipantKind
  display_label: string
  role_label?: string | null
  /**
   * Tenant org for internal members, counterparty id for external contacts —
   * **different axes**. Comparing this without checking `kind` compares two
   * things that were never the same kind of identifier.
   */
  affiliation_id?: string | null
  affiliation_label?: string | null
}

/**
 * One mail or messenger thread.
 *
 * There is deliberately no message-body field. The server projects only a
 * bounded `last_message_preview` and never the stored full text.
 */
export interface ContextHomeThread {
  thread_id: string
  channel_kind: string
  subject: string
  message_count: number
  has_external_participants: boolean
  last_message_at?: string | null
  last_message_preview: string
  last_message_sender_id?: string | null
  last_message_sender_kind?: ParticipantKind | null
  participants: ContextHomeParticipant[]
  participant_count: number
  project_id?: string | null
  project_label?: string | null
}

export interface ContextHomeProject {
  project_id: string
  name: string
  code?: string | null
  status?: string | null
  project_kind?: string | null
  my_role?: string | null
  member_count?: number | null
  started_on?: string | null
  target_end_on?: string | null
  counterparty_id?: string | null
  counterparty_label?: string | null
}

export interface ContextHomeThreadSection {
  status: SectionStatus
  items: ContextHomeThread[]
  truncated: boolean
  next_cursor?: string | null
  unavailable_reason?: SectionUnavailableReason | null
}

export interface ContextHomeProjectSection {
  status: SectionStatus
  items: ContextHomeProject[]
  truncated: boolean
  next_cursor?: string | null
  unavailable_reason?: SectionUnavailableReason | null
}

/**
 * Who the **server** resolved the request as — echoed back from the JWT, never
 * from anything this client sent.
 */
export interface ContextHomeActor {
  actor_id: string
  organization_id: string
}

export interface ContextHomeProvenance {
  synthetic_only: boolean
  seed_namespaces: string[]
  seed_revisions: string[]
}

export interface ContextHomeSnapshot {
  contract_version: string
  snapshot_id: string
  as_of: string
  /**
   * IANA zone the server intends the dates to be read in. Explicit because the
   * demo's day boundaries are KST and the viewer's local zone is not guaranteed
   * to match — formatting `as_of` in browser-local time would silently move
   * items across day boundaries.
   */
  timezone: string
  actor: ContextHomeActor
  synthetic: boolean
  provenance: ContextHomeProvenance
  mail: ContextHomeThreadSection
  messenger: ContextHomeThreadSection
  projects: ContextHomeProjectSection
}

/**
 * Wire codes this surface can reject with.
 *
 * All are ADR-019 registry codes, so `translateError` has a localized template
 * for each and nothing here needs an entry in `TAURI_IPC_ERROR_KEYS`.
 *
 * `sessionExpired` and `permissionDenied` are the split this slice exists for.
 * They are not interchangeable: re-login resolves the first and does nothing for
 * the second, so a surface that offers "sign in again" for `policy.denied`
 * hands the user a loop with no exit.
 */
export const CONTEXT_HOME_ERROR_CODES = {
  /** Session gone. Re-login is the fix. */
  sessionExpired: 'auth.failed',
  /** Authenticated but not permitted. Re-login will NOT help. */
  permissionDenied: 'policy.denied',
  /** Transient — server fault, or no transport wired in this build. */
  unavailable: 'service.unavailable',
  /** Transient — retry is sane. */
  timeout: 'network.timeout',
  /** The response was absent, oversized, or not a valid snapshot. */
  invalidResponse: 'validation.invalid_field',
} as const

export type ContextHomeErrorCode = (typeof CONTEXT_HOME_ERROR_CODES)[keyof typeof CONTEXT_HOME_ERROR_CODES]

/**
 * Raised when the IPC bridge itself is absent — the standalone browser
 * dashboard rather than the Tauri webview.
 *
 * Deliberately NOT an `IpcError` with an invented code: `TAURI_IPC_ERROR_KEYS`
 * is pinned to exactly the out-of-registry codes *Rust* mints, so a code minted
 * here could not be mapped there and would fall through `translateError` to its
 * raw-`message` fallback — an English literal in a Korean UI (#9492).
 */
export class ContextHomeBridgeUnavailableError extends Error {
  constructor() {
    super('no desktop IPC bridge in this context')
    this.name = 'ContextHomeBridgeUnavailableError'
  }
}

/** True when `err` is the given context-home failure (or any of them). */
export function isContextHomeError(err: unknown, code?: ContextHomeErrorCode): boolean {
  if (!isIpcError(err)) return false
  return code ? err.code === code : Object.values(CONTEXT_HOME_ERROR_CODES).includes(err.code as ContextHomeErrorCode)
}

/**
 * True when retrying the same call could plausibly succeed.
 *
 * Exposed so a surface does not have to re-derive the retryable set and get
 * `policy.denied` wrong — retrying a permission denial produces the same answer
 * forever.
 */
export function isRetryableContextHomeError(err: unknown): boolean {
  return (
    isContextHomeError(err, CONTEXT_HOME_ERROR_CODES.unavailable) ||
    isContextHomeError(err, CONTEXT_HOME_ERROR_CODES.timeout)
  )
}

/**
 * Fetch the signed-in actor's context-home snapshot.
 *
 * Rejects with the typed `IpcError` envelope (see [`CONTEXT_HOME_ERROR_CODES`])
 * or with [`ContextHomeBridgeUnavailableError`] when there is no desktop bridge.
 *
 * No retry here on purpose: the caller owns the retry policy because only the
 * caller knows whether a stale snapshot is still on screen.
 */
export async function fetchContextHome(): Promise<ContextHomeSnapshot> {
  let core: typeof import('@tauri-apps/api/core')
  try {
    core = await import('@tauri-apps/api/core')
  } catch {
    throw new ContextHomeBridgeUnavailableError()
  }
  return core.invoke<ContextHomeSnapshot>('fetch_context_home')
}
