import { describe, expect, it } from 'vitest'

import { buildBugReportIssueUrl, buildMailtoUrl } from '../bug-report'
import type { BugReportBundle } from '../contracts'

const legacySupportAddress = 'support@' + 'oneshim.dev'
const legacyReportName = 'oneshim' + '-report-bug_123.json'

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
})
