/**
 * Tauri IPC error envelope type + type guard.
 *
 * Mirrors the Rust `src-tauri/src/ipc_error.rs::IpcError` DTO introduced
 * by ADR-019 Follow-up #1. Rust-side commands that have been migrated
 * from `Result<T, String>` to `Result<T, IpcError>` serialize failures
 * as `{ code: string, message: string }`, which surfaces on the JS side
 * via the `.catch()` clause of `invoke()` or `invokeDesktop()`.
 *
 * See:
 * - [ADR-019](../../../../docs/architecture/ADR-019-error-code-infrastructure.md)
 * - [Follow-up #1 design](../../../../docs/reviews/2026-04-20-adr019-followup-ipc-error-dto-design.md)
 *
 * @example
 * ```ts
 * import { isIpcError } from "@/api/desktop";
 *
 * invoke<Response>("some_command").catch((err) => {
 *   if (isIpcError(err)) {
 *     if (err.code === "config.invalid") { ... }
 *     else if (err.code.startsWith("network.")) { ... }
 *     else { console.error(err.code, err.message); }
 *   } else {
 *     // Command not yet migrated — err is still a string.
 *     console.error("legacy string error:", err);
 *   }
 * });
 * ```
 *
 * Post-migration (iter-196~204 migrated every command-decorated function; iter-204 milestone counted 114/17, current `grep -cE "#\[(tauri::)?command\]"` shows 106/19),
 * the typed envelope is the norm. The `isIpcError` type guard + plain-string
 * fallback below remain for defence-in-depth — e.g., future commands that
 * haven't been converted, or legacy callers running during a rollback window.
 * Do NOT assume every thrown Tauri error matches this shape.
 */
export interface IpcError {
  readonly code: string
  readonly message: string
}

/**
 * Type guard that narrows an unknown Tauri invoke rejection to `IpcError`.
 *
 * A Tauri error from a migrated command is an object with exactly two
 * string fields (`code` and `message`). Commands that have not yet
 * migrated still surface plain strings, so the guard also handles the
 * "not yet migrated" case by returning false.
 */
export function isIpcError(x: unknown): x is IpcError {
  return (
    typeof x === 'object' &&
    x !== null &&
    'code' in x &&
    'message' in x &&
    typeof (x as { code: unknown }).code === 'string' &&
    typeof (x as { message: unknown }).message === 'string'
  )
}

/**
 * Extract a user-displayable message from an unknown Tauri invoke error.
 *
 * Prefers the `message` field on a typed `IpcError`. Falls back to plain
 * string errors (pre-migration commands), then to `Error.message`, then
 * to the default string representation. Never returns empty — callers
 * can always assign the result directly to UI state.
 *
 * Note: this is deliberately NOT localized. For localized user-facing
 * messages, use the i18n translator from Follow-up #3 (see
 * `docs/reviews/2026-04-20-adr019-followup-frontend-i18n-wiring-design.md`),
 * which keys off `IpcError.code`.
 */
export function errorMessageFromInvoke(err: unknown): string {
  if (isIpcError(err)) {
    return err.message
  }
  if (typeof err === 'string') {
    return err
  }
  if (err instanceof Error) {
    return err.message
  }
  return String(err)
}
