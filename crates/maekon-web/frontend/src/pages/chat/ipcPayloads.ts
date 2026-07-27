/**
 * Tauri commands with a Rust `request` parameter require one matching
 * top-level key. Keep that boundary explicit so request fields cannot drift
 * into positional invoke arguments.
 */
export function buildSendSessionMessageArgs<T extends Record<string, unknown>>(request: T): { request: T } {
  return { request }
}
