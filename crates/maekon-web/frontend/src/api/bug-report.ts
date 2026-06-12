import { withLocalAuthHeaders } from '../utils/api-base'
import type { BugReportBundle, ProviderCliDiagnosticSummary } from './contracts'

const BASE_URL = '/api'

export async function createBugReport(includeLogs = true, piiLevel?: string): Promise<BugReportBundle> {
  // E20-41 (#4833): carry the X-Local-Auth header (the local-API gate). This is a
  // raw fetch outside the client.ts chokepoint, so attach it explicitly.
  const res = await fetch(
    `${BASE_URL}/support/bug-report`,
    withLocalAuthHeaders({
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        include_logs: includeLogs,
        pii_level: piiLevel ?? null,
      }),
    }),
  )
  if (!res.ok) throw new Error(`Bug report failed: ${res.status}`)
  return sanitizeBugReportBundleForDisplay(await res.json())
}

export type ClipboardFormat = 'json' | 'text'

const EMAIL_PATTERN = /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/gi
const PROVIDER_SECRET_PATTERN = /\b(?:sk|pk|rk)_(?:live|test)_[A-Za-z0-9_-]+\b/g

export function sanitizeSupportText(text: string): string {
  return text.replace(EMAIL_PATTERN, '[EMAIL]').replace(PROVIDER_SECRET_PATTERN, '[PROVIDER_SECRET]')
}

function executableFileNameHint(value: string): string {
  const pathParts = value.split(/[\\/]/).filter(Boolean)
  return pathParts[pathParts.length - 1] ?? value
}

function sanitizeProviderCliDiagnostic(summary: ProviderCliDiagnosticSummary): ProviderCliDiagnosticSummary {
  return {
    ...summary,
    surface_id: sanitizeSupportText(summary.surface_id),
    tool_id: summary.tool_id ? sanitizeSupportText(summary.tool_id) : summary.tool_id,
    candidate_name: summary.candidate_name ? sanitizeSupportText(summary.candidate_name) : summary.candidate_name,
    executable_hint: summary.executable_hint
      ? sanitizeSupportText(executableFileNameHint(summary.executable_hint))
      : summary.executable_hint,
    dependency_status: summary.dependency_status,
    status_reason: summary.status_reason ? sanitizeSupportText(summary.status_reason) : summary.status_reason,
  }
}

export function sanitizeBugReportBundleForDisplay(bundle: BugReportBundle): BugReportBundle {
  return {
    ...bundle,
    diagnostics: {
      ...bundle.diagnostics,
      provider_cli: (bundle.diagnostics.provider_cli ?? []).map(sanitizeProviderCliDiagnostic),
    },
    runtime_logs: bundle.runtime_logs
      ? {
          ...bundle.runtime_logs,
          log_dir: sanitizeSupportText(bundle.runtime_logs.log_dir),
          log_file: bundle.runtime_logs.log_file
            ? sanitizeSupportText(bundle.runtime_logs.log_file)
            : bundle.runtime_logs.log_file,
          recent_text: sanitizeSupportText(bundle.runtime_logs.recent_text),
        }
      : bundle.runtime_logs,
  }
}

export function formatBundleForClipboard(bundle: BugReportBundle, format: ClipboardFormat): string {
  const safeBundle = sanitizeBugReportBundleForDisplay(bundle)
  if (format === 'json') {
    return JSON.stringify(safeBundle, null, 2)
  }
  const runtimeLogLines = safeBundle.runtime_logs
    ? [
        '',
        `--- Runtime Logs (${safeBundle.runtime_logs.line_count}) ---`,
        `Generated: ${safeBundle.runtime_logs.generated_at}`,
        safeBundle.runtime_logs.log_file ? `Log File: ${safeBundle.runtime_logs.log_file}` : '',
        safeBundle.runtime_logs.recent_text || 'No runtime log text captured.',
      ]
    : []
  const providerCli = safeBundle.diagnostics.provider_cli ?? []
  const providerCliLines =
    providerCli.length > 0
      ? [
          '',
          `--- Provider CLI Diagnostics (${providerCli.length}) ---`,
          ...providerCli.flatMap((entry) =>
            [
              `${entry.surface_id}: ${entry.readiness} / ${entry.availability}`,
              entry.dependency_status ? `  Dependency: ${entry.dependency_status}` : '',
              entry.env_refresh_required ? '  Restart Required: true' : '',
              entry.status_reason ? `  Reason: ${entry.status_reason}` : '',
            ].filter(Boolean),
          ),
        ]
      : []

  return [
    '=== Maekon Bug Report ===',
    `Bug ID: ${safeBundle.bug_id}`,
    `Generated: ${safeBundle.diagnostics.generated_at}`,
    '',
    '--- System ---',
    `App Version: ${safeBundle.system.app_version}`,
    `OS: ${safeBundle.system.os_name} ${safeBundle.system.os_version} (${safeBundle.system.arch})`,
    `Runtime: ${safeBundle.system.runtime}`,
    `CPU: ${safeBundle.system.cpu_count} cores`,
    `Memory: ${safeBundle.system.memory_available_mb}/${safeBundle.system.memory_total_mb} MB`,
    '',
    '--- Health ---',
    `Storage OK: ${safeBundle.diagnostics.health.storage_ok}`,
    `Frames Dir: ${safeBundle.diagnostics.health.frames_dir_exists ?? 'unknown'}`,
    '',
    '--- Connection ---',
    `Server: ${safeBundle.connection.server_reachable ? 'reachable' : 'unreachable'}`,
    `Last Sync: ${safeBundle.connection.last_sync_at ?? 'never'}`,
    `gRPC: ${safeBundle.connection.grpc_enabled ? 'enabled' : 'disabled'}`,
    ...providerCliLines,
    ...runtimeLogLines,
    '',
    `--- Recent Audit (${safeBundle.diagnostics.recent_audit_entries.length}) ---`,
    ...safeBundle.diagnostics.recent_audit_entries
      .slice(0, 10)
      .map((e) => `  [${e.timestamp}] ${e.action_type}: ${e.status}`),
    safeBundle.diagnostics.recent_audit_entries.length > 10
      ? `  ... and ${safeBundle.diagnostics.recent_audit_entries.length - 10} more`
      : '',
  ]
    .filter(Boolean)
    .join('\n')
}

const ISSUE_REPO = 'https://github.com/pseudotop/maekon-client/issues/new'

export function buildBugReportIssueUrl(bundle: BugReportBundle): string {
  const safeBundle = sanitizeBugReportBundleForDisplay(bundle)
  const providerCli = safeBundle.diagnostics.provider_cli ?? []
  const providerCliLines =
    providerCli.length > 0
      ? [
          '',
          '## Provider CLI',
          ...providerCli.map((entry) => `- ${entry.surface_id}: ${entry.readiness} / ${entry.availability}`),
        ]
      : []
  const params = new URLSearchParams({
    title: `Bug report: ${safeBundle.system.app_version}`,
    body: [
      '## Summary',
      '<!-- Describe the issue here -->',
      '',
      '## Bug ID',
      `\`${safeBundle.bug_id}\``,
      '',
      '## Environment',
      `- App version: ${safeBundle.system.app_version}`,
      `- Runtime: ${safeBundle.system.runtime}`,
      `- OS: ${safeBundle.system.os_name} ${safeBundle.system.os_version} (${safeBundle.system.arch})`,
      `- Storage OK: ${safeBundle.diagnostics.health.storage_ok}`,
      `- Connection: ${safeBundle.connection.server_reachable ? 'server reachable' : 'server unreachable'}`,
      ...providerCliLines,
      '',
      '## Reproduction',
      '1. ',
      '',
      '## Expected',
      '',
      '## Actual',
      '',
      '## Notes',
      '- If you exported a diagnostic report, please email it to support@maekon.dev with this Bug ID in the subject line.',
    ].join('\n'),
  })
  return `${ISSUE_REPO}?${params.toString()}`
}

export function buildMailtoUrl(bugId: string): string {
  const subject = encodeURIComponent(`Bug Report ${bugId}`)
  const body = encodeURIComponent(
    `Bug ID: ${bugId}\n\nPlease attach the exported diagnostic report (maekon-report-${bugId}.json) to this email.\n\nDescribe the issue:\n`,
  )
  return `mailto:support@maekon.dev?subject=${subject}&body=${body}`
}
