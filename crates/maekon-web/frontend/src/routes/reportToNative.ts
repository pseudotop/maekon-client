import { logFrontend } from '../logging/frontendLogger'
import { IS_TAURI } from '../utils/platform'
import { redact } from './redact'

export type Severity = 'warning' | 'error' | 'critical'

interface ReportOptions {
  route: string
  severity: Severity
  message: string
  stack?: string
  componentStack?: string
}

/**
 * Fire-and-forget error report to the Rust backend.
 *
 * Two-pipeline design:
 *  - Step 1: forward to `frontendLogger` (existing IPC bridge), which calls
 *    `record_frontend_log` for structured logging via tracing.
 *  - Step 2: invoke the dedicated `report_frontend_error` command for the
 *    severity classification, native notification, and recovery dispatch.
 *
 * Both pipelines exist because they have distinct side effects:
 *  - `record_frontend_log` is the canonical structured log writer.
 *  - `report_frontend_error` triggers desktop notifications and emits
 *    `frontend-recovery` events (which `record_frontend_log` does not).
 *
 * Both steps are async and fire-and-forget — never blocks the UI.
 *
 * Defense-in-depth:
 *  - All string inputs are passed through `redact()` to mask file paths and
 *    common secret patterns BEFORE leaving the JS context.
 *  - Standalone/dev mode (no Tauri) falls through to console.error so the
 *    error is still surfaced to the developer's DevTools.
 */
export function reportToNative(options: ReportOptions): void {
  const { route, severity } = options

  // Apply PII redaction before any logging
  const message = redact(options.message) ?? ''
  const stack = redact(options.stack)
  const componentStack = redact(options.componentStack)

  // Standalone/dev mode fallback — surface the error to DevTools
  if (!IS_TAURI) {
    // eslint-disable-next-line no-console
    console.error(`[route-error:${severity}] ${route}: ${message}`, {
      stack,
      componentStack,
    })
    return
  }

  // Step 1: log through the existing frontend logger bridge (record_frontend_log)
  const context = [stack, componentStack].filter(Boolean).join('\n---componentStack---\n')
  logFrontend('route-error-boundary', severity === 'warning' ? 'warn' : 'error', message, context || undefined)

  // Step 2: invoke the dedicated error report command for recovery
  void (async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      // Tauri auto-converts camelCase JS → snake_case Rust:
      // errorMessage → error_message, componentStack → component_stack
      await invoke('report_frontend_error', {
        route,
        severity,
        errorMessage: message,
        stack: stack ?? null,
        componentStack: componentStack ?? null,
      })
    } catch {
      // Standalone/dev mode — Tauri command unavailable, ignore silently
    }
  })()
}
