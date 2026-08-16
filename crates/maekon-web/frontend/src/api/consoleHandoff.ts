/** PII-free receipt returned after Maekon opens the fixed Console entry route. */
export interface ConsoleHandoffReceipt {
  contract_version: 'console-handoff.v1'
  run_id: string
  source_snapshot_id: string
  issued_at: string
  expires_at: string
  synthetic: true
  source_provenance: {
    seed_namespaces: string[]
    seed_revisions: string[]
  }
}

export const CONSOLE_HANDOFF_ERROR_CODES = {
  sessionExpired: 'auth.failed',
  permissionDenied: 'policy.denied',
  unavailable: 'service.unavailable',
  timeout: 'network.timeout',
  rateLimit: 'network.rate_limit',
  invalidResponse: 'validation.invalid_field',
  configMissing: 'config.missing',
  configInvalid: 'config.invalid',
  rejected: 'handoff.rejected',
  noHandler: 'handoff.no_handler',
  launchFailed: 'handoff.launch_failed',
} as const

export class ConsoleHandoffBridgeUnavailableError extends Error {
  constructor() {
    super('no desktop IPC bridge in this context')
    this.name = 'ConsoleHandoffBridgeUnavailableError'
  }
}

/**
 * Opens the assignment board without accepting a URL, actor, organization, or
 * bearer from the WebView. Rust resolves all authority from the active JWT.
 */
export async function openConsoleAssignmentBoard(): Promise<ConsoleHandoffReceipt> {
  let core: typeof import('@tauri-apps/api/core')
  try {
    core = await import('@tauri-apps/api/core')
  } catch {
    throw new ConsoleHandoffBridgeUnavailableError()
  }
  return core.invoke<ConsoleHandoffReceipt>('open_console_assignment_board')
}
