/**
 * #8044: capture-history viewing re-authentication (biometric/PIN) IPC wrapper.
 *
 * When entering the captured screenshot timeline/replay views, this
 * performs an OS biometric (Touch ID) or app PIN re-auth. The backend
 * `require_capture_reauth` middleware gates capture-history surfaces
 * separately from the session token (fail-closed) — only a successful
 * re-auth opens the shared gate.
 *
 * Rust counterpart: `src-tauri/src/commands/reauth.rs`, `maekon-core::reauth`.
 */

import { isApiErrorCode } from './client'
import type { ExportDataType } from './contracts'

async function invokeDesktop<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

/** Re-authentication method. */
export type ReauthMethod = 'biometric' | 'pin'

/**
 * Result of a re-authentication attempt (tagged union). Only `authenticated`
 * opens the gate. Matches the Rust `ReauthOutcome`'s
 * `#[serde(tag="outcome", content="detail")]` shape.
 */
export type ReauthOutcome =
  | { outcome: 'authenticated' }
  | { outcome: 'cancelled' }
  | { outcome: 'failed'; detail: string }
  | { outcome: 'unsupported' }

/** Re-authentication status + platform capabilities + PIN enrollment state. */
export interface ReauthStatus {
  /** Whether the re-auth gate is enabled (config value). `false` means viewing is always allowed. */
  readonly enabled: boolean
  /** Idle expiry (seconds). */
  readonly idle_timeout_secs: number
  /** Whether a currently-valid re-auth session exists (i.e. whether viewing is allowed right now). */
  readonly authenticated: boolean
  /** Whether OS biometrics are currently available. */
  readonly biometric_available: boolean
  /** Biometric method name ("Touch ID" etc). `null` when unavailable. */
  readonly biometric_kind: string | null
  /** Whether an app PIN fallback is enrolled. */
  readonly pin_enrolled: boolean
}

/** Only capture metadata exports are protected by the capture-history gate. */
export function requiresCaptureReauthForExport(dataType: ExportDataType, error: unknown): boolean {
  return dataType === 'frames' && isApiErrorCode(error, 'auth.reauth_required')
}

/** Fetches the current re-authentication status. */
export function getCaptureReauthStatus(): Promise<ReauthStatus> {
  return invokeDesktop<ReauthStatus>('get_capture_reauth_status')
}

/** Performs re-authentication to view capture history (biometric or PIN). */
export function authenticateCaptureHistory(request: {
  method: ReauthMethod
  pin?: string
  reason?: string
}): Promise<ReauthOutcome> {
  return invokeDesktop<ReauthOutcome>('authenticate_capture_history', { request })
}

/** Enrolls/updates the app PIN fallback. */
export function registerCaptureReauthPin(pin: string): Promise<void> {
  return invokeDesktop<void>('register_capture_reauth_pin', { pin })
}

/** Removes the enrolled app PIN fallback. */
export function clearCaptureReauthPin(): Promise<void> {
  return invokeDesktop<void>('clear_capture_reauth_pin')
}

/** Immediately locks the re-auth session (app foreground re-entry / manual lock). */
export function lockCaptureReauth(): Promise<void> {
  return invokeDesktop<void>('lock_capture_reauth')
}

/**
 * Updates the re-auth configuration (enabled / idle expiry) — persists to
 * config and applies to the live gate immediately. The toggle takes effect
 * without a restart.
 */
export function setCaptureReauthConfig(enabled: boolean, idleTimeoutSecs: number): Promise<void> {
  return invokeDesktop<void>('set_capture_reauth_config', {
    enabled,
    idleTimeoutSecs,
  })
}
