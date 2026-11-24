import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { I18nextProvider } from 'react-i18next'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { fetchHourlyMetrics, fetchProcesses, fetchSummary } from '../../api/client'
import { ThemeProvider } from '../../contexts/ThemeContext'
import type { IdleUpdate } from '../../hooks/useSSE'
import i18n from '../../i18n'
import { LocalizedError } from '../../i18n/errorDisplay'
import { AppMemoryRouter } from '../../router/future'
import { formatDuration } from '../../utils/formatters'
import MonitoringSection from './MonitoringSection'

const sse = vi.hoisted(() => ({ idleState: null as IdleUpdate | null }))
const clients: QueryClient[] = []
const generic = {
  en: 'Something went wrong. Try again.',
  ko: '문제가 발생했습니다. 다시 시도하세요.',
  ja: '問題が発生しました。もう一度お試しください。',
  'zh-CN': '出现了问题。请重试。',
  es: 'Algo salió mal. Inténtelo de nuevo.',
}
const policy = {
  en: 'Policy denied',
  ko: '정책에 의해 거부되었습니다',
  ja: 'ポリシーにより拒否されました',
  'zh-CN': '被策略拒绝',
  es: 'Denegado por la política',
}
type Language = keyof typeof generic

async function mountFailure(language: Language) {
  await i18n.changeLanguage(language)
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  clients.push(client)
  render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>
        <ThemeProvider>
          <AppMemoryRouter>
            <MonitoringSection />
          </AppMemoryRouter>
        </ThemeProvider>
      </QueryClientProvider>
    </I18nextProvider>,
  )
  return client
}

afterEach(async () => {
  clients.forEach((client) => {
    client.clear()
  })
  clients.length = 0
  vi.restoreAllMocks()
  await i18n.changeLanguage('en')
})

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>()
  return { ...actual, fetchSummary: vi.fn(), fetchHourlyMetrics: vi.fn(), fetchProcesses: vi.fn() }
})

vi.mock('../../hooks/useSSE', () => ({
  useSSE: () => ({
    status: 'connected',
    latestMetrics: {
      timestamp: '2026-11-10T10:00:00Z',
      cpu_usage: 25.3,
      memory_percent: 50.7,
      memory_used: 4_000_000_000,
      memory_total: 8_000_000_000,
    },
    latestFrame: null,
    idleState: sse.idleState,
    metricsHistory: [],
    connect: vi.fn(),
    disconnect: vi.fn(),
  }),
}))

// Charts need a layout engine jsdom does not have; this page only places them.
vi.mock('../../components/MetricsChart', () => ({ default: () => <div data-testid="metrics-chart" /> }))
vi.mock('../../components/AppUsageChart', () => ({ default: () => <div data-testid="app-usage-chart" /> }))

beforeEach(() => {
  localStorage.clear()
  vi.clearAllMocks()
  sse.idleState = null
  vi.mocked(fetchSummary).mockResolvedValue({
    date: '2026-11-10',
    total_active_secs: 3600,
    total_idle_secs: 600,
    top_apps: [],
    cpu_avg: 20,
    memory_avg_percent: 40,
    frames_captured: 1234,
    events_logged: 56,
  })
  vi.mocked(fetchHourlyMetrics).mockResolvedValue([])
  vi.mocked(fetchProcesses).mockResolvedValue([])
})

describe('MonitoringSection as its own page (#13735)', () => {
  it('renders outside the Dashboard layout with its own title and data', async () => {
    // No DashboardContext outlet around it: the page must not depend on one.
    renderWithProviders(<MonitoringSection />)

    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(i18n.t('sidebar.systemMetrics'))
    expect(screen.getByTestId('monitoring-realtime')).toHaveTextContent('25.3%')
    expect(document.getElementById('section-metrics')).toBeInTheDocument()
    expect(document.getElementById('section-processes')).toBeInTheDocument()
    await waitFor(() => expect(screen.getByTestId('metric-card-captures-value')).toHaveTextContent('1,234'))
    expect(screen.getByTestId('metric-card-events-value')).toHaveTextContent('56')
  })

  it("reads today's summary for the capture counts", async () => {
    renderWithProviders(<MonitoringSection />)
    await waitFor(() => expect(fetchSummary).toHaveBeenCalledTimes(1))

    const [date] = vi.mocked(fetchSummary).mock.calls[0]
    expect(date).toMatch(/^\d{4}-\d{2}-\d{2}$/)
  })

  it('shows how long the user has been idle in the shared duration format', () => {
    sse.idleState = { is_idle: true, idle_secs: 5 * 60 + 40 }
    renderWithProviders(<MonitoringSection />)

    expect(screen.getByTestId('monitoring-idle-badge').textContent).toBe(
      `${i18n.t('dashboard.idle')} ${formatDuration(5 * 60 + 40, true)}`,
    )
  })
})

describe('MonitoringSection actual error display and retry (#13756)', () => {
  const panels = [
    { query: 'hourlyMetrics', fetch: fetchHourlyMetrics, args: [24] },
    { query: 'processes', fetch: fetchProcesses, args: [undefined, undefined, 5] },
  ] as const

  it.each(
    panels.flatMap((panel) =>
      (Object.keys(generic) as Language[]).flatMap((language) =>
        ['plain', 'future.metrics', 'policy.denied'].map((code) => ({ panel, language, code })),
      ),
    ),
  )('keeps diagnostics collapsed, copies them and retries $panel.query in $language ($code)', async ({
    panel,
    language,
    code,
  }) => {
    const message = 'English metric diagnostic sentinel'
    const failure = code === 'plain' ? new Error(message) : { code, message }
    vi.mocked(panel.fetch).mockRejectedValue(failure)
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const client = await mountFailure(language)
    await waitFor(() => expect(client.getQueryState([panel.query])?.status).toBe('error'))
    expect(panel.fetch).toHaveBeenCalledTimes(1)
    expect(panel.fetch).toHaveBeenCalledWith(...panel.args)
    const query = client.getQueryCache().find({ queryKey: [panel.query], exact: true })!
    const alert = await screen.findByRole('alert')
    const primary = within(alert).getByText(code === 'policy.denied' ? policy[language] : generic[language])
    expect(primary).not.toHaveTextContent(message)
    const details = within(alert).getByTestId('support-details')
    expect(details).not.toHaveAttribute('open')
    fireEvent.click(details.querySelector('summary')!)
    fireEvent.click(within(details).getByRole('button'))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(code === 'plain' ? message : `${code}: ${message}`))

    vi.mocked(panel.fetch).mockResolvedValue([])
    fireEvent.click(alert.querySelector('button')!)
    await waitFor(() => expect(query.state.status).toBe('success'))
    expect(client.getQueryCache().find({ queryKey: [panel.query], exact: true })).toBe(query)
    expect(panel.fetch).toHaveBeenCalledTimes(2)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.getByTestId('monitoring-realtime')).toHaveTextContent('25.3%')
  })

  it.each(panels)('shows null rejection for $query and recovers the same query', async (panel) => {
    vi.mocked(panel.fetch).mockRejectedValue(null)
    const client = await mountFailure('ja')
    await waitFor(() => expect(client.getQueryState([panel.query])?.status).toBe('error'))
    expect(panel.fetch).toHaveBeenCalledTimes(1)
    const query = client.getQueryCache().find({ queryKey: [panel.query], exact: true })!
    const alert = await screen.findByRole('alert')
    expect(within(alert).getByText(generic.ja)).toBeInTheDocument()
    expect(within(alert).queryByTestId('support-details')).not.toBeInTheDocument()
    vi.mocked(panel.fetch).mockResolvedValue([])
    fireEvent.click(within(alert).getByRole('button'))
    await waitFor(() => expect(query.state.status).toBe('success'))
    expect(panel.fetch).toHaveBeenCalledTimes(2)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it.each(panels)('keeps an already localized $query error without support detail', async (panel) => {
    const message = '이미 번역한 오류 문구'
    vi.mocked(panel.fetch).mockRejectedValue(new LocalizedError(message))
    await mountFailure('ko')
    const alert = await screen.findByRole('alert')
    expect(panel.fetch).toHaveBeenCalledTimes(1)
    expect(within(alert).getByText(message)).toBeInTheDocument()
    expect(within(alert).queryByTestId('support-details')).not.toBeInTheDocument()
  })
})
