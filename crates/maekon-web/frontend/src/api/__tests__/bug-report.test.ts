import { describe, expect, it } from 'vitest'

import { buildBugReportIssueUrl, buildMailtoUrl, formatBundleForClipboard } from '../bug-report'
import type { BugReportBundle } from '../contracts'

// Regression guards: assert no legacy oneshim brand strings leak into the
// generated GitHub issue / mailto URLs. String concatenation is intentional —
// keeps these literals out of bundler brand-rename greps so future rebrand
// passes don't rewrite the negative-assertion target.
const legacySupportAddress = 'support@' + 'oneshim.dev'
const legacyReportName = 'oneshim' + '-report-bug_123.json'
const providerSecretFixture = 'sk_' + 'live_abc123'

const bundle = {
  bug_id: 'bug_123',
  diagnostics: {
    generated_at: '2026-05-03T00:00:00Z',
    health: { storage_ok: true },
  },
  system: {
    app_version: '0.4.41',
    os_name: 'macOS',
    os_version: '15.4',
    arch: 'aarch64',
    runtime: 'desktop',
  },
  connection: {
    server_reachable: false,
  },
} as BugReportBundle

describe('bug report links', () => {
  it('routes issue reports and diagnostic follow-up to Maekon support', () => {
    const url = decodeURIComponent(buildBugReportIssueUrl(bundle))

    expect(url).toContain('https://github.com/pseudotop/maekon-client/issues/new')
    expect(url).toContain('support@maekon.dev')
    expect(url).not.toContain(legacySupportAddress)
  })

  it('uses Maekon report names in the support email body', () => {
    const url = decodeURIComponent(buildMailtoUrl('bug_123'))

    expect(url).toContain('mailto:support@maekon.dev')
    expect(url).toContain('maekon-report-bug_123.json')
    expect(url).not.toContain(legacyReportName)
  })

  it('includes sanitized runtime log excerpts in human-readable clipboard text', () => {
    const text = formatBundleForClipboard(
      {
        ...bundle,
        diagnostics: {
          ...bundle.diagnostics,
          recent_audit_entries: [],
          recent_policy_events: [],
        },
        runtime_logs: {
          generated_at: '2026-05-25T16:30:00Z',
          log_dir: '[USER]/Library/Logs/com.maekon.app',
          log_file: '[USER]/Library/Logs/com.maekon.app/maekon.log',
          line_count: 2,
          recent_text: `provider failed for alice@example.com\nendpoint token ${providerSecretFixture}`,
        },
      },
      'text',
    )

    expect(text).toContain('--- Runtime Logs (2) ---')
    expect(text).toContain('provider failed for [EMAIL]')
    expect(text).toContain('endpoint token [PROVIDER_SECRET]')
    expect(text).not.toContain('alice@example.com')
    expect(text).not.toContain(providerSecretFixture)
  })

  it('includes sanitized provider CLI diagnostics in clipboard text and issue URLs', () => {
    const cliPath = 'C:\\Users\\alice\\AppData\\Local\\Programs\\Codex\\codex.exe'
    const bodyWithProviderCli = {
      ...bundle,
      diagnostics: {
        ...bundle.diagnostics,
        recent_audit_entries: [],
        recent_policy_events: [],
        provider_cli: [
          {
            surface_id: 'provider_surface.openai.subprocess_cli',
            tool_id: 'codex',
            candidate_name: 'codex',
            executable_hint: cliPath,
            readiness: 'auth_required',
            availability: 'partially_available',
            dependency_status: 'ready',
            status_reason: `auth failed for alice@example.com with ${providerSecretFixture}`,
            env_refresh_required: false,
          },
        ],
      },
    } as unknown as BugReportBundle

    const text = formatBundleForClipboard(bodyWithProviderCli, 'text')
    const issueBody = new URL(buildBugReportIssueUrl(bodyWithProviderCli)).searchParams.get('body') ?? ''

    expect(text).toContain('--- Provider CLI Diagnostics (1) ---')
    expect(text).toContain('provider_surface.openai.subprocess_cli: auth_required / partially_available')
    expect(text).toContain('Dependency: ready')
    expect(text).toContain('Reason: auth failed for [EMAIL] with [PROVIDER_SECRET]')
    expect(text).not.toContain('alice@example.com')
    expect(text).not.toContain(providerSecretFixture)
    expect(text).not.toContain(cliPath)

    expect(issueBody).toContain('- provider_surface.openai.subprocess_cli: auth_required / partially_available')
    expect(issueBody).not.toContain('alice@example.com')
    expect(issueBody).not.toContain(providerSecretFixture)
    expect(issueBody).not.toContain(cliPath)
  })
})
