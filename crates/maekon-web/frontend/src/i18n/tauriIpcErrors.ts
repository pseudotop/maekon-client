/**
 * UI translations for the `IpcError` codes minted *above* `maekon-core`.
 *
 * `translateError` reads `wire-errors.*.json`, and that catalog is locked
 * key-for-key to the ADR-019 registry in
 * `crates/maekon-core/tests/wire_contract_snapshot.expected.txt` — the
 * `has exactly the same key count as the registry` assertion in
 * `__tests__/translateError.test.ts` fails on any extra key. The codes below
 * are `IpcError::new(..)` literals minted in `src-tauri/src/commands/*.rs`,
 * not `CoreError` variants, so they are absent from that registry and would
 * otherwise hit translateError's raw-`message` fallback — i.e. a Rust English
 * literal shown verbatim to a Korean user. Mapping them to UI-namespaced keys
 * here keeps the wire catalog honest while still localizing the message.
 *
 * The seven entries are the complete set of out-of-registry Tauri-layer codes
 * on this HEAD; `is_err_hedge`-style drift is caught by
 * `__tests__/tauriIpcErrors.test.ts`, which pins every mapped key across all
 * five locale files.
 *
 * - `auth.*` (`commands/auth.rs`) — introduced by #9459.
 * - `storage.unavailable` (`commands/consent.rs::erase_all_local_data`) —
 *   GDPR Art. 17 erasure could not verify frame deletion. #9492 folded it in;
 *   it was the last of the four still rendering its Rust literal.
 * - `handoff.*` (`commands/os_handoff.rs`) — introduced by #9707. These carry
 *   Rust-side detail that is deliberately unfit for display:
 *   `handoff.rejected`'s message names the policy rule that refused the target,
 *   which is a sentence for a log and not for a user, and `handoff.launch_failed`
 *   embeds a raw OS exit status. Both are mapped rather than passed through.
 *
 * `auth.feature_disabled` reuses the notice the no-server-build branch already
 * renders rather than duplicating that sentence across five locale files.
 */

import { type IpcError, isIpcError } from '../api/desktop'
import { translateError, type WireErrorLocale } from './translateError'

export const TAURI_IPC_ERROR_KEYS: Readonly<Record<string, string>> = {
  'auth.invalid_arguments': 'settings.account.login.errors.invalidArguments',
  'auth.token_manager_unavailable': 'settings.account.login.errors.tokenManagerUnavailable',
  'auth.feature_disabled': 'settings.account.login.notAvailable',
  'storage.unavailable': 'errors.ipc.storageUnavailable',
  'handoff.rejected': 'errors.ipc.handoffRejected',
  'handoff.no_handler': 'errors.ipc.handoffNoHandler',
  'handoff.launch_failed': 'errors.ipc.handoffLaunchFailed',
}

/**
 * Wire-error locale for the active UI language.
 *
 * Deliberately narrow: every surface that routes through `translateError`
 * (AudioTab, SupportToolsCard, BugReportWizard, the overlay panels) resolves
 * ko-or-English the same way, so this helper only centralizes the existing
 * house rule. `translateError` itself covers all five runtime locales, so
 * broadening this (ja / zh-CN / es) is a standalone follow-up rather than a
 * side effect of any single slice.
 */
export function wireLocaleFor(language: string | undefined): WireErrorLocale {
  return language?.split('-')[0] === 'ko' ? 'ko' : 'en'
}

/** Default lookup: the plain wire-code → UI-key table above. */
function defaultLookup(err: IpcError): string | undefined {
  return TAURI_IPC_ERROR_KEYS[err.code]
}

/**
 * Localize an unknown Tauri invoke rejection for display.
 *
 * Resolution order: the caller's UI-key lookup first (out-of-registry Tauri
 * codes), then `translateError` (ADR-019 registry codes, then the raw
 * `message` fallback for anything else).
 *
 * `lookup` lets a surface narrow a code that means different things in
 * different places — see [`loginFormIpcErrorKey`], where `auth.failed` during a
 * sign-in attempt is a rejected submission rather than "the session you had is
 * gone".
 */
export function describeIpcError(
  err: unknown,
  translate: (key: string) => string,
  language: string | undefined,
  lookup: (err: IpcError) => string | undefined = defaultLookup,
): string {
  const key = isIpcError(err) ? lookup(err) : undefined
  return key ? translate(key) : translateError(err, wireLocaleFor(language))
}

/**
 * UI keys reachable only from the sign-in form (see [`loginFormIpcErrorKey`]).
 *
 * Exported so the locale-parity suite covers them: they live outside
 * `TAURI_IPC_ERROR_KEYS` and would otherwise be in no parity loop at all.
 */
export const LOGIN_FORM_IPC_ERROR_KEYS = {
  invalidCredentials: 'settings.account.login.errors.invalidCredentials',
  rejected: 'settings.account.login.errors.rejected',
} as const

/**
 * Matches the login-failure message for a 401, as the webview actually receives
 * it.
 *
 * Two layers of formatting stand between `TokenManager::login_with_org` and
 * this string, and getting either wrong makes the branch dead code:
 *
 * 1. `login_with_org` builds `login failure ({status}): {capped body}`, where
 *    `{status}` is `reqwest::StatusCode`'s Display — `401 Unauthorized`
 *    (`crates/maekon-network/src/auth/mod.rs`).
 * 2. `IpcError::from(CoreError)` then stores `err.to_string()`, i.e. the
 *    `thiserror` Display of `CoreError::Auth`:
 *    `Authentication error [{code}]: {message}`
 *    (`crates/maekon-core/src/error.rs`, `src-tauri/src/ipc_error.rs`).
 *
 * So the wire value is
 * `Authentication error [auth.failed]: login failure (401 Unauthorized): {body}`
 * — the same nesting `providerErrorGuidance.test.ts` documents for the chat
 * surface. The prefix is optional so a future un-wrapped `IpcError::new` with
 * the same payload still matches, but the pattern stays anchored so an
 * adversarial response body cannot smuggle the marker in from the tail.
 *
 * `commands::auth::tests::ipc_error_message_shape_matches_frontend_401_matcher`
 * pins the exact Rust-side string this expects.
 */
const LOGIN_FAILURE_401 = /^(?:Authentication error \[auth\.failed\]: )?login failure \(401\b/

/**
 * Login-form view of an `IpcError` code (#9492 item 1).
 *
 * `auth.failed` is the wire code `login_with_org` mints for every non-transient
 * login failure — rejected credentials, a server-side 500, and connection
 * failures alike — and its message embeds the server's (length-capped) response
 * body. Rendering that through `translateError`'s `{message}` slot puts a raw
 * RFC 7807 JSON blob in the toast.
 *
 * Two reasons this mapping is not folded into `TAURI_IPC_ERROR_KEYS`:
 * `auth.failed` is a registry code with a legitimate catalog template, and
 * outside a sign-in attempt it means "the session you had is gone", which is a
 * different sentence.
 *
 * The 401 split keeps the common case precise without claiming "wrong password"
 * about a 500 or an unreachable server. If either Rust-side format changes, the
 * match stops firing and the generic sentence takes over — the raw body never
 * reaches the toast either way.
 */
export function loginFormIpcErrorKey(err: IpcError): string | undefined {
  if (err.code === 'auth.failed') {
    return LOGIN_FAILURE_401.test(err.message)
      ? LOGIN_FORM_IPC_ERROR_KEYS.invalidCredentials
      : LOGIN_FORM_IPC_ERROR_KEYS.rejected
  }
  return TAURI_IPC_ERROR_KEYS[err.code]
}
