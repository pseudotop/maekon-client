import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { createInstance } from 'i18next'
import { I18nextProvider } from 'react-i18next'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import type { UpdatePhase, UpdateStatus } from '../api/client'
import i18n from '../i18n'
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

import UpdatePanel from './UpdatePanel'

const mockFetchUpdateStatus = vi.fn()
const mockPostUpdateAction = vi.fn()
const mockUseUpdateStream = vi.fn()

vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>()
  return {
    ...actual,
    fetchUpdateStatus: () => mockFetchUpdateStatus(),
    postUpdateAction: (...args: unknown[]) => mockPostUpdateAction(...args),
  }
})

vi.mock('../hooks/useUpdateStream', () => ({
  useUpdateStream: () => mockUseUpdateStream(),
}))

function status(phase: UpdatePhase, updatedAt: string): UpdateStatus {
  return {
    enabled: true,
    auto_install: false,
    phase,
    message: 'Already on latest version: 0.4.41-rc.1',
    pending: null,
    download_progress: null,
    rollback: null,
    revision: 1,
    updated_at: updatedAt,
  }
}

describe('UpdatePanel', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    mockFetchUpdateStatus.mockReset()
    mockUseUpdateStream.mockReset()
    mockUseUpdateStream.mockReturnValue({
      status: 'connected',
      latest: undefined,
      recoveredAt: null,
      lastError: null,
      retryCount: 0,
    })
  })

  it('does not show approval-blocked copy for stale non-actionable update status', async () => {
    mockFetchUpdateStatus.mockResolvedValue(status('Updated', '2000-01-01T00:00:00.000Z'))

    renderWithProviders(<UpdatePanel />)

    await waitFor(() => {
      expect(screen.getByText('Already on latest version: 0.4.41-rc.1')).toBeInTheDocument()
    })

    expect(screen.queryByText(/approval is temporarily blocked/i)).not.toBeInTheDocument()
  })

  it('keeps approval-blocked copy when a stale actionable update is waiting', async () => {
    mockFetchUpdateStatus.mockResolvedValue(status('PendingApproval', '2000-01-01T00:00:00.000Z'))

    renderWithProviders(<UpdatePanel />)

    await waitFor(() => {
      expect(screen.getByText(/approval is temporarily blocked/i)).toBeInTheDocument()
    })
  })

  it('announces one neutral recovery status without rendering a duplicate warning', async () => {
    mockUseUpdateStream.mockReturnValue({
      status: 'connected',
      latest: undefined,
      recoveredAt: Date.now(),
      lastError: null,
      retryCount: 0,
    })
    mockFetchUpdateStatus.mockResolvedValue(status('Updated', new Date().toISOString()))

    renderWithProviders(<UpdatePanel />)

    const recovery = await screen.findByText(/live update stream connection was restored/i)
    expect(recovery).toHaveAttribute('role', 'status')
    expect(screen.queryByText(/temporary issues/i)).not.toBeInTheDocument()
    expect(screen.getAllByRole('status')).toHaveLength(1)
  })

  // #13740: a bare "Polling" / "Fresh" / "Idle" said nothing about what it described.
  it('names what each status pill describes and explains it on hover', async () => {
    mockFetchUpdateStatus.mockResolvedValue(status('Idle', new Date().toISOString()))

    renderWithProviders(<UpdatePanel />)

    const connection = await screen.findByText('Connection: Live')
    expect(connection).toHaveAttribute('title', expect.stringMatching(/how this panel receives update status/i))
    expect(screen.getByText('Signal: Recent')).toHaveAttribute('title', expect.stringMatching(/arrived recently/i))
    expect(screen.getByText('Stage: Idle')).toHaveAttribute('title', expect.stringMatching(/checking, downloading/i))
  })

  it('labels the pills in Korean too', async () => {
    mockUseUpdateStream.mockReturnValue({
      status: 'disconnected',
      latest: undefined,
      recoveredAt: null,
      lastError: null,
      retryCount: 0,
    })
    mockFetchUpdateStatus.mockResolvedValue(status('Idle', '2000-01-01T00:00:00.000Z'))
    await i18n.changeLanguage('ko')

    renderWithProviders(<UpdatePanel />)

    expect(await screen.findByText('연결: 주기 확인')).toBeInTheDocument()
    expect(screen.getByText('신호: 지연')).toBeInTheDocument()
    expect(screen.getByText('단계: 대기')).toBeInTheDocument()
  })

  it('states the signal age with the shared duration format', async () => {
    const updatedAt = new Date(Date.now() - (3600 + 125) * 1000).toISOString()
    mockFetchUpdateStatus.mockResolvedValue(status('Idle', updatedAt))

    renderWithProviders(<UpdatePanel />)

    expect(await screen.findByText('Last update signal: 1h 2m ago')).toBeInTheDocument()
  })

  it('pluralizes the release age in English', async () => {
    const pending = status('PendingApproval', new Date().toISOString())
    pending.pending = {
      current_version: '0.0.1-rc.11',
      latest_version: '0.0.1-rc.12',
      release_url: 'https://example.invalid/release',
      release_name: null,
      published_at: new Date(Date.now() - 5 * 60_000 - 10_000).toISOString(),
      download_url: 'https://example.invalid/download',
    }
    mockFetchUpdateStatus.mockResolvedValue(pending)

    renderWithProviders(<UpdatePanel />)

    expect(await screen.findByText('Latest: 0.0.1-rc.12 (released 5 minutes ago)')).toBeInTheDocument()
  })

  it('offers Check now unless the page header already does', async () => {
    mockFetchUpdateStatus.mockResolvedValue(status('Idle', new Date().toISOString()))

    const checkNow = i18n.t('updates.checkNow')
    const { unmount } = renderWithProviders(<UpdatePanel />)
    expect(await screen.findAllByRole('button', { name: checkNow })).toHaveLength(1)
    unmount()

    renderWithProviders(<UpdatePanel hideCheckNow />)
    await screen.findByText(i18n.t('updates.phaseBadge', { state: i18n.t('updates.idle') }))
    expect(screen.queryByRole('button', { name: checkNow })).not.toBeInTheDocument()
  })

  it('shows Korean recovery copy and keeps raw updater failures collapsed as technical details', async () => {
    const failed = status('Error', new Date().toISOString())
    failed.message = 'Failed to check for updates: Failed to parse API response: 404 Not Found'
    mockFetchUpdateStatus.mockResolvedValue(failed)
    await i18n.changeLanguage('ko')

    renderWithProviders(<UpdatePanel />)

    await waitFor(() => {
      expect(
        screen.getByText('업데이트 정보를 가져오지 못했습니다. 네트워크 또는 서버 상태를 확인한 뒤 다시 시도하세요.'),
      ).toBeInTheDocument()
    })

    const details = screen.getByTestId('support-details')
    expect(details).not.toHaveAttribute('open')
    expect(details.querySelector('summary')).toHaveTextContent(i18n.t('errors.supportDetails'))
    expect(screen.getByText(failed.message)).toBeInTheDocument()
  })
})

// #13756: drive the actual mutation, cache recovery and shared diagnostic copy.
describe('UpdatePanel display contract', () => {
  const clients: QueryClient[] = []
  afterEach(() => {
    clients.forEach((client) => {
      client.clear()
    })
    clients.length = 0
    vi.restoreAllMocks()
  })
  it.each(
    Object.keys(resources).flatMap((language) =>
      ['future.update', 'policy.denied'].map((code) => ({ language, code })),
    ),
  )('keeps structured action diagnostics out of the primary in %s and recovers', async ({ language, code }) => {
    const locale = await testLocale(language)
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
    clients.push(client)
    const current = status('Idle', new Date().toISOString())
    mockFetchUpdateStatus.mockResolvedValue(current)
    mockUseUpdateStream.mockReturnValue({
      status: 'connected',
      latest: undefined,
      recoveredAt: null,
      lastError: null,
      retryCount: 0,
    })
    mockPostUpdateAction
      .mockReset()
      .mockRejectedValueOnce({ code, message: 'structured updater diagnostic' })
      .mockResolvedValue({ status: current })
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    render(
      <I18nextProvider i18n={locale}>
        <QueryClientProvider client={client}>
          <UpdatePanel />
        </QueryClientProvider>
      </I18nextProvider>,
    )
    await waitFor(() => expect(client.getQueryData(['update-status'])).toEqual(current))
    fireEvent.click(screen.getByRole('button'))
    const alert = await screen.findByRole('alert')
    expect(alert.querySelector('p')).toHaveTextContent(
      code === 'policy.denied' ? policyPrimary[language] : locale.t('updates.actionFailed'),
    )
    expect(alert.querySelector('p')).not.toHaveTextContent('structured updater diagnostic')
    const detail = within(alert).getByTestId('support-details')
    expect(detail).not.toHaveAttribute('open')
    expect(within(detail).getByTestId('support-details-text')).toHaveTextContent(
      `${code}: structured updater diagnostic`,
    )
    fireEvent.click(detail.querySelector('summary')!)
    fireEvent.click(within(detail).getByRole('button'))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(`${code}: structured updater diagnostic`))
    fireEvent.click(screen.getAllByRole('button')[0])
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument())
    expect(mockPostUpdateAction.mock.calls).toEqual([['CheckNow'], ['CheckNow']])
    expect(client.getQueryData(['update-status'])).toEqual(current)
  })
})
