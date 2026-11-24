import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { createInstance } from 'i18next'
import { I18nextProvider } from 'react-i18next'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import type { BugReportBundle } from '../api/contracts'
import en from '../i18n/locales/en.json'
import es from '../i18n/locales/es.json'
import ja from '../i18n/locales/ja.json'
import ko from '../i18n/locales/ko.json'
import zhCN from '../i18n/locales/zh-CN.json'

const policyPrimary: Record<string, string> = {
  en: 'Policy denied',
  ko: '정책에 의해 거부되었습니다',
  ja: 'ポリシーにより拒否されました',
  'zh-CN': '被策略拒绝',
  es: 'Denegado por la política',
}
const resources = {
  en: { translation: en },
  ko: { translation: ko },
  ja: { translation: ja },
  'zh-CN': { translation: zhCN },
  es: { translation: es },
}
async function testLocale(language = 'en') {
  const locale = createInstance()
  await locale.init({ lng: language, fallbackLng: 'en', resources, interpolation: { escapeValue: false } })
  return locale
}

import { clearToasts, useToast } from '../hooks/useToast'
import BugReportWizard from './BugReportWizard'
import { ToastContainer } from './ui/Toast'

const invoke = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('../utils/platform', () => ({ IS_TAURI: true, IS_MAC: false, IS_WINDOWS: false, IS_LINUX: true }))

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

function ToastProbe() {
  const { toasts } = useToast()
  return <output data-testid="toast-records">{JSON.stringify(toasts)}</output>
}
describe('BugReportWizard export failure', () => {
  afterEach(() => {
    clearToasts()
    vi.restoreAllMocks()
  })
  it.each(
    Object.keys(resources).flatMap((language) =>
      ['future.export', 'policy.denied'].map((code) => ({ language, code })),
    ),
  )('exports sanitized data and keeps native diagnostics in support detail in %s', async ({ language, code }) => {
    clearToasts()
    const locale = await testLocale(language)
    mockCreateBugReport.mockResolvedValue(bugReportBundle)
    invoke.mockReset().mockRejectedValueOnce({ code, message: 'native export diagnostic' }).mockResolvedValueOnce(null)
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    render(
      <I18nextProvider i18n={locale}>
        <BugReportWizard open onClose={() => undefined} />
        <ToastContainer />
        <ToastProbe />
      </I18nextProvider>,
    )
    const dialog = screen.getByRole('dialog')
    // The generate and next actions are the unique primary buttons in their steps.
    fireEvent.click(dialog.querySelector('button.bg-brand')!)
    await waitFor(() => expect(mockCreateBugReport).toHaveBeenCalled())
    await waitFor(() => expect(dialog.querySelector('button.bg-brand')).not.toBeDisabled())
    await screen.findByText('bug_runtime_logs')
    fireEvent.click(dialog.querySelector('button.bg-brand')!)
    const actions = dialog.querySelectorAll('button.text-left')
    expect(actions).toHaveLength(5)
    fireEvent.click(actions[3])
    const detail = await screen.findByTestId('support-details')
    expect(detail).not.toHaveAttribute('open')
    expect(screen.getByTestId('toast-records')).toHaveTextContent(
      code === 'policy.denied' ? policyPrimary[language] : locale.t('settings.bugReportExportFailed'),
    )
    const records = JSON.parse(screen.getByTestId('toast-records').textContent!)
    expect(records).toEqual([
      expect.objectContaining({
        type: 'error',
        message: code === 'policy.denied' ? policyPrimary[language] : locale.t('settings.bugReportExportFailed'),
        duration: 5000,
        detail: `${code}: native export diagnostic`,
      }),
    ])
    expect(invoke).toHaveBeenCalledWith('export_bug_report', expect.objectContaining({ bugId: 'bug_runtime_logs' }))
    const payload = invoke.mock.calls[0][1].bundleJson
    expect(payload).toContain('[PROVIDER_SECRET]')
    expect(payload).not.toContain('alice@example.com')
    expect(payload).not.toContain(providerSecretFixture)
    fireEvent.click(detail.querySelector('summary')!)
    fireEvent.click(within(detail).getByRole('button'))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(`${code}: native export diagnostic`))
    clearToasts()
    await waitFor(() => expect(actions[3]).not.toBeDisabled())
    fireEvent.click(actions[3])
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(actions[3]).not.toBeDisabled())
    expect(screen.getByTestId('toast-records')).toHaveTextContent('[]')
  })
})
