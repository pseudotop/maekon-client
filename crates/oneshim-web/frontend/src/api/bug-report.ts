import type { BugReportBundle } from './contracts'

const BASE_URL = '/api'

export async function createBugReport(includeLogs = true, piiLevel?: string): Promise<BugReportBundle> {
  const res = await fetch(`${BASE_URL}/support/bug-report`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      include_logs: includeLogs,
      pii_level: piiLevel ?? null,
    }),
  })
  if (!res.ok) throw new Error(`Bug report failed: ${res.status}`)
  return res.json()
}

export type ClipboardFormat = 'json' | 'text'

export function formatBundleForClipboard(bundle: BugReportBundle, format: ClipboardFormat): string {
  if (format === 'json') {
    return JSON.stringify(bundle, null, 2)
  }
  return [
    '=== Maekon Bug Report ===',
    `Bug ID: ${bundle.bug_id}`,
    `Generated: ${bundle.diagnostics.generated_at}`,
    '',
    '--- System ---',
    `App Version: ${bundle.system.app_version}`,
    `OS: ${bundle.system.os_name} ${bundle.system.os_version} (${bundle.system.arch})`,
    `Runtime: ${bundle.system.runtime}`,
    `CPU: ${bundle.system.cpu_count} cores`,
    `Memory: ${bundle.system.memory_available_mb}/${bundle.system.memory_total_mb} MB`,
    '',
    '--- Health ---',
    `Storage OK: ${bundle.diagnostics.health.storage_ok}`,
    `Frames Dir: ${bundle.diagnostics.health.frames_dir_exists ?? 'unknown'}`,
    '',
    '--- Connection ---',
    `Server: ${bundle.connection.server_reachable ? 'reachable' : 'unreachable'}`,
    `Last Sync: ${bundle.connection.last_sync_at ?? 'never'}`,
    `gRPC: ${bundle.connection.grpc_enabled ? 'enabled' : 'disabled'}`,
    '',
    `--- Recent Audit (${bundle.diagnostics.recent_audit_entries.length}) ---`,
    ...bundle.diagnostics.recent_audit_entries
      .slice(0, 10)
      .map((e) => `  [${e.timestamp}] ${e.action_type}: ${e.status}`),
    bundle.diagnostics.recent_audit_entries.length > 10
      ? `  ... and ${bundle.diagnostics.recent_audit_entries.length - 10} more`
      : '',
  ]
    .filter(Boolean)
    .join('\n')
}

const ISSUE_REPO = 'https://github.com/pseudotop/maekon-client/issues/new'

export function buildBugReportIssueUrl(bundle: BugReportBundle): string {
  const params = new URLSearchParams({
    title: `Bug report: ${bundle.system.app_version}`,
    body: [
      '## Summary',
      '<!-- Describe the issue here -->',
      '',
      '## Bug ID',
      `\`${bundle.bug_id}\``,
      '',
      '## Environment',
      `- App version: ${bundle.system.app_version}`,
      `- Runtime: ${bundle.system.runtime}`,
      `- OS: ${bundle.system.os_name} ${bundle.system.os_version} (${bundle.system.arch})`,
      `- Storage OK: ${bundle.diagnostics.health.storage_ok}`,
      `- Connection: ${bundle.connection.server_reachable ? 'server reachable' : 'server unreachable'}`,
      '',
      '## Reproduction',
      '1. ',
      '',
      '## Expected',
      '',
      '## Actual',
      '',
      '## Notes',
      '- If you exported a diagnostic report, please email it to support@oneshim.dev with this Bug ID in the subject line.',
    ].join('\n'),
  })
  return `${ISSUE_REPO}?${params.toString()}`
}

export function buildMailtoUrl(bugId: string): string {
  const subject = encodeURIComponent(`Bug Report ${bugId}`)
  const body = encodeURIComponent(
    `Bug ID: ${bugId}\n\nPlease attach the exported diagnostic report (oneshim-report-${bugId}.json) to this email.\n\nDescribe the issue:\n`,
  )
  return `mailto:support@oneshim.dev?subject=${subject}&body=${body}`
}
