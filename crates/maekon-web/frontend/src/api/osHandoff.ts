/**
 * OS handoff IPC surface — the sanctioned way to leave the app (#9707).
 *
 * The Rust side (`src-tauri/src/commands/os_handoff.rs`) owns the allowlist:
 * `https` only for allowlisted hosts, `mailto` only for RFC 2606/6761 reserved
 * domains, and a refusal for any control character or encoded CR/LF. That
 * placement is the point — a recipient rule enforced in this file would live on
 * the side of the boundary the rule is supposed to constrain, and #9627 needs a
 * recipient that its own UI cannot widen.
 *
 * Callers get three distinguishable outcomes: resolved (handed off), a rejection
 * code they should treat as a bug in their own URL construction, and the two
 * runtime failures a user can actually hit. `describeIpcError` already maps all
 * three to localized sentences.
 *
 * This module deliberately does not wrap `window.open`. The webview CSP is
 * `default-src 'self'` with `connect-src` limited to `127.0.0.1:10090-10099`
 * (`src-tauri/tauri.conf.json`), so an external target was never reachable by
 * navigating the webview; and `window.open`'s `null` return cannot separate "no
 * mail app" from "handoff failed", which is the distinction #9627 has to act on.
 */

import { isIpcError } from './desktop'

/** Wire codes minted by `commands/os_handoff.rs`. */
export const HANDOFF_ERROR_CODES = {
  /** The target failed the Rust-side policy. Reaching this is a caller bug. */
  rejected: 'handoff.rejected',
  /** Nothing is registered for the scheme — typically no mail client. */
  noHandler: 'handoff.no_handler',
  /** A handler may exist, but the OS did not complete the request. */
  launchFailed: 'handoff.launch_failed',
} as const

export type HandoffErrorCode = (typeof HANDOFF_ERROR_CODES)[keyof typeof HANDOFF_ERROR_CODES]

/**
 * Raised when the IPC bridge itself is absent — the standalone browser
 * dashboard rather than the Tauri webview.
 *
 * Deliberately NOT an `IpcError` with a `handoff.*` code. `TAURI_IPC_ERROR_KEYS`
 * is pinned to exactly the out-of-registry codes *Rust* mints
 * (`__tests__/tauriIpcErrors.test.ts`), so a code invented here could not be
 * mapped there, and an unmapped `IpcError` falls through `translateError` to its
 * raw-`message` fallback — an English literal in a Korean UI, which is the
 * #9492 defect. A distinct class instead lets each surface decide its own
 * sentence, because "you cannot open links from the browser dashboard" is a
 * different thing to say in Settings than in a mail-draft dialog.
 */
export class HandoffBridgeUnavailableError extends Error {
  constructor() {
    super('no desktop IPC bridge in this context')
    this.name = 'HandoffBridgeUnavailableError'
  }
}

/** True when `err` is one of the Rust-side handoff failures. */
export function isHandoffError(err: unknown, code?: HandoffErrorCode): boolean {
  if (!isIpcError(err)) return false
  return code ? err.code === code : Object.values(HANDOFF_ERROR_CODES).includes(err.code as HandoffErrorCode)
}

/**
 * Hand `url` to the user's default application for its scheme.
 *
 * Resolves once the OS has accepted the request. Rejects with the typed
 * `IpcError` envelope otherwise (see [`HANDOFF_ERROR_CODES`]), or with
 * [`HandoffBridgeUnavailableError`] when there is no desktop bridge at all.
 *
 * One call opens at most one window. There is no retry here on purpose: a
 * retry loop around an OS handoff is how one user gesture becomes several
 * compose windows.
 */
export async function openExternalTarget(url: string): Promise<void> {
  let core: typeof import('@tauri-apps/api/core')
  try {
    core = await import('@tauri-apps/api/core')
  } catch {
    throw new HandoffBridgeUnavailableError()
  }
  await core.invoke<void>('open_external_target', { url })
}
