import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import type { BugReportBundle } from '../api/contracts'
import BugReportWizard from './BugReportWizard'

const mockCreateBugReport = vi.fn()
const providerSecretFixture = 'sk_' + 'live_abc123'

vi.mock('../api/bug-report', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/bug-report')>()
  return {
    ...actual,
    createBugReport: (...args: unknown[]) => mockCreateBugReport(...args),
  }
})

const bugReportBundle = {
  bug_id: 'bug_runtime_logs',
  diagnostics: {
    generated_at: '2026-05-25T16:30:00Z',
    health: { storage_ok: true },
    provider_cli: [
      {
        surface_id: 'provider_surface.openai.subprocess_cli',
        tool_id: 'codex',
        candidate_name: 'codex',
        executable_hint: 'C:\\Users\\alice\\AppData\\Local\\Programs\\Codex\\codex.exe',
        readiness: 'auth_required',
        availability: 'partially_available',
        dependency_status: 'ready',
        status_reason: `auth failed for alice@example.com with ${providerSecretFixture}`,
        env_refresh_required: false,
      },
    ],
    recent_audit_entries: [],
    recent_policy_events: [],
  },
  system: {
    app_version: '0.0.1-rc.5',
    os_name: 'macOS',
    os_version: '15.5',
    arch: 'aarch64',
    runtime: 'desktop',
    cpu_count: 10,
    memory_total_mb: 24576,
    memory_available_mb: 8192,
    uptime_seconds: 120,
  },
  connection: {
    server_reachable: true,
    last_sync_at: null,
    grpc_enabled: true,
    websocket_connected: false,
  },
  runtime_logs: {
    generated_at: '2026-05-25T16:29:59Z',
    log_dir: '[USER]/Library/Logs/com.maekon.app',
    log_file: '[USER]/Library/Logs/com.maekon.app/maekon.log',
    line_count: 2,
    recent_text: `provider failed for alice@example.com\nendpoint token ${providerSecretFixture}`,
  },
  pii_filter_level: 'standard',
} as unknown as BugReportBundle

describe('BugReportWizard', () => {
  beforeEach(() => {
    mockCreateBugReport.mockReset()
  })

  it('shows sanitized runtime log excerpts in the review step before sharing', async () => {
    mockCreateBugReport.mockResolvedValue(bugReportBundle)

    renderWithProviders(<BugReportWizard open onClose={() => undefined} />)

    fireEvent.click(screen.getByRole('button', { name: /Generate Report/i }))

    await waitFor(() => {
      expect(screen.getByText('bug_runtime_logs')).toBeInTheDocument()
    })

    expect(screen.getByText(/provider failed for \[EMAIL\]/)).toBeInTheDocument()
    expect(screen.getByText(/endpoint token \[PROVIDER_SECRET\]/)).toBeInTheDocument()
    expect(screen.getByText(/provider_surface\.openai\.subprocess_cli/)).toBeInTheDocument()
    expect(screen.getByText(/auth_required \/ partially_available/)).toBeInTheDocument()
    expect(screen.getByText(/auth failed for \[EMAIL\] with \[PROVIDER_SECRET\]/)).toBeInTheDocument()
    expect(screen.queryByText(/alice@example\.com/)).not.toBeInTheDocument()
    expect(screen.queryByText(providerSecretFixture)).not.toBeInTheDocument()
    expect(screen.queryByText(/C:\\Users\\alice/)).not.toBeInTheDocument()
  })
})
