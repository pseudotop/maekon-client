/**
 * Sign-in IPC surface shared by every surface that can authenticate this device
 * (#9603 WD-02.1).
 *
 * Before this module the wire shape, the "IPC bridge is absent" fallback and the
 * two `invoke` calls lived inline in `setting-tabs/GeneralTab.tsx`. The demo
 * login screen needs exactly the same three, and a second copy is how the two
 * surfaces would drift — one of them keeping a stale field name after the Rust
 * DTO changes, with nothing failing until a user tries to sign in.
 *
 * The Rust side is `src-tauri/src/commands/auth.rs`; both commands are
 * `#[cfg(feature = "server")]` dual-path, so a build without the `server`
 * feature answers `auth_status` with `server_feature: false` and rejects
 * `login` with `auth.feature_disabled` rather than 404-ing the command.
 */

import { clearSyntheticSession } from '../shell/syntheticSessionSignal'

/**
 * Wire shape of `AuthStatusResponse` (`src-tauri/src/commands/auth.rs`).
 *
 * Field names are snake_case verbatim — the Rust struct carries no
 * `serde(rename_all)`, and `auth_status_response_serializes_exact_snake_case_wire_keys`
 * pins that. `identifier`/`organization_id` are null, never absent, when signed out.
 */
export interface AuthStatus {
  server_feature: boolean
  authenticated: boolean
  identifier: string | null
  organization_id: string | null
}

/** Credentials the `login` command takes, in the camelCase form Tauri v2 expects. */
export interface LoginCredentials {
  identifier: string
  password: string
  organizationId: string
}

/**
 * What we assume when `auth_status` cannot be reached at all — i.e. the dynamic
 * `@tauri-apps/api/core` import or the invoke itself rejects, which is the
 * normal case in the standalone browser dashboard. Presenting it as "this build
 * has no connected mode" degrades to the same read-only notice a non-server
 * Tauri build gets, instead of offering a form that could never submit.
 */
export const SERVER_UNREACHABLE: AuthStatus = {
  server_feature: false,
  authenticated: false,
  identifier: null,
  organization_id: null,
}

async function invokeDesktop<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

/** Read this device's session state. Infallible Rust-side; rejects only if the IPC bridge is absent. */
export function readAuthStatus(): Promise<AuthStatus> {
  return invokeDesktop<AuthStatus>('auth_status')
}

/**
 * Attempt a sign-in and return this device's view of the session it created.
 *
 * The password leaves the caller only here — it is never logged, echoed into a
 * toast, or retained once the call settles. Tauri v2 converts the camelCase keys
 * to the command's snake_case parameters.
 */
export function requestLogin({ identifier, password, organizationId }: LoginCredentials): Promise<AuthStatus> {
  return invokeDesktop<AuthStatus>('login', { identifier, password, organizationId })
}

/**
 * Revoke every session for this account, including this device's.
 *
 * Also drops the shell's latched "synthetic demo data" claim (#9611). That
 * claim is scoped to a session — carrying it into the next sign-in would mean
 * labelling a connection to a real server as demo data on no evidence at all.
 * Cleared even if the revocation call rejects: the local session is gone either
 * way, and a label about data we can no longer fetch is not a claim we can
 * still support.
 */
export async function requestLogoutAllSessions(): Promise<void> {
  try {
    await invokeDesktop<void>('logout_all_sessions')
  } finally {
    clearSyntheticSession()
  }
}
