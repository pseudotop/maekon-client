import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, type RenderResult, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { ReactElement } from 'react'
import { I18nextProvider } from 'react-i18next'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as api from '../../api/client'
import { DEFAULT_PROVIDER_SURFACE_CATALOG } from '../../api/defaultProviderSurfaceCatalog'
import { getCaptureReauthStatus } from '../../api/reauth'
import { ToastContainer } from '../../components/ui/Toast'
import * as toast from '../../hooks/useToast'
import i18n from '../../i18n'
import type { SettingsDataResult } from '../hooks/useSettingsData'
import { useSettingsForm } from '../hooks/useSettingsForm'
import DataStorageTab from './DataStorageTab'
import { makeDefaultFormData } from './stories-utils'

// The tab hosts TagManagementCard, which reads the shared ['tags'] query.
// Keep the real component mounted (a missing provider here would mirror a real
// regression) but serve it an empty tag list instead of a network call.
vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  fetchTags: vi.fn().mockResolvedValue([]),
  exportData: vi.fn(),
  downloadBlob: vi.fn(),
}))

function renderTab(ui: ReactElement): RenderResult {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>{ui}</MemoryRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  )
}

const mocks = vi.hoisted(() => ({
  useSettingsFormContext: vi.fn(),
  useLoadedFormData: vi.fn(),
}))

vi.mock('../settings/SettingsFormContext', () => ({
  useSettingsFormContext: mocks.useSettingsFormContext,
  useLoadedFormData: mocks.useLoadedFormData,
}))

vi.mock('../../api/reauth', async (original) => ({
  ...(await original<typeof import('../../api/reauth')>()),
  getCaptureReauthStatus: vi.fn(),
}))

describe('DataStorageTab capture export re-authentication', () => {
  const handleExport = vi.fn()
  const dismissExportReauth = vi.fn()

  beforeEach(() => {
    handleExport.mockReset()
    dismissExportReauth.mockReset()
    mocks.useLoadedFormData.mockReturnValue({
      retention_days: 30,
      max_storage_mb: 500,
      telemetry: {
        enabled: false,
        crash_reports: false,
        usage_analytics: false,
        performance_metrics: false,
      },
    })
    mocks.useSettingsFormContext.mockReturnValue({
      form: {
        exportFormat: 'json',
        exportLoading: null,
        exportRecovery: null,
        exportReauthStatus: {
          enabled: true,
          idle_timeout_secs: 300,
          authenticated: false,
          biometric_available: false,
          biometric_kind: null,
          pin_enrolled: true,
        },
        setExportFormat: vi.fn(),
        handleExport,
        dismissExportReauth,
        resumeFrameExport: vi.fn(),
        handleRootChange: vi.fn(),
        handleTelemetryChange: vi.fn(),
      },
      data: {
        storageStats: null,
        storageLoading: false,
      },
    })
  })

  it('wires the frame export action and renders the shared PIN dialog', async () => {
    const user = userEvent.setup()
    renderTab(<DataStorageTab />)

    await user.click(screen.getByRole('button', { name: new RegExp(i18n.t('settings.exportFramesLabel')) }))
    expect(handleExport).toHaveBeenCalledWith('frames')
    expect(screen.getByRole('dialog', { name: i18n.t('reauth.title') })).toBeInTheDocument()
    expect(screen.getByLabelText(i18n.t('reauth.pinLabel'))).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: i18n.t('reauth.cancel') }))
    expect(dismissExportReauth).toHaveBeenCalledOnce()
  })

  it('keeps a storage export failure visible and retries only the failed export', async () => {
    const user = userEvent.setup()
    mocks.useSettingsFormContext.mockReturnValue({
      form: {
        exportFormat: 'json',
        exportLoading: null,
        exportReauthStatus: null,
        exportRecovery: {
          dataType: 'metrics',
          display: { text: 'Storage error', detail: 'storage.failed: disk diagnostic' },
          storageFailure: true,
        },
        setExportFormat: vi.fn(),
        handleExport,
        dismissExportReauth,
        resumeFrameExport: vi.fn(),
        handleRootChange: vi.fn(),
        handleTelemetryChange: vi.fn(),
      },
      data: {
        storageStats: null,
        storageLoading: false,
      },
    })

    renderTab(<DataStorageTab />)

    const alert = screen.getByRole('alert')
    expect(alert).toHaveTextContent(i18n.t('settings.exportFailed'))
    expect(alert).toHaveTextContent('storage.failed')
    expect(alert).toHaveTextContent(i18n.t('settings.exportStorageRecovery'))
    expect(alert).not.toHaveTextContent(i18n.t('settings.exportDone'))

    await user.click(screen.getByRole('button', { name: i18n.t('settings.exportRetry') }))
    expect(handleExport).toHaveBeenCalledWith('metrics')
  })
})

const actualData = {
  settings: undefined,
  settingsLoading: false,
  storageStats: undefined,
  storageLoading: false,
  updateStatus: undefined,
  featureCapabilities: undefined,
  desktopPermissionStatus: undefined,
  desktopPermissionStatusError: null,
  desktopPermissionStatusLoading: false,
  desktopPermissionStatusRefreshing: false,
  secretBackendCapabilities: undefined,
  canQueryDesktopCapabilities: false,
  providerCatalog: DEFAULT_PROVIDER_SURFACE_CATALOG,
  ocrEndpointProbe: null,
  ocrEndpointProbeLoading: false,
  llmEndpointProbe: null,
  llmEndpointProbeLoading: false,
  defaultByokBackendKind: 'file',
  handleRefreshDesktopPermissionStatus: vi.fn(),
  requestNotificationPermissionRefetch: vi.fn(),
  deriveEndpointAuthMode: () => 'none',
  deriveEndpointBackendKind: () => 'file',
  defaultExternalApiSettings: vi.fn(),
  normalizeEndpointSettings: (_mode, _kind, endpoint) => endpoint ?? null,
  resolveSurfaceForState: () => undefined,
} satisfies SettingsDataResult

function ActualExportTab() {
  const form = useSettingsForm(actualData)
  mocks.useSettingsFormContext.mockReturnValue({ form, data: actualData })
  mocks.useLoadedFormData.mockReturnValue(makeDefaultFormData())
  return (
    <>
      <DataStorageTab />
      <ToastContainer />
    </>
  )
}

describe('actual export hook and consumer recovery (#13756)', () => {
  afterEach(async () => {
    toast.clearToasts()
    vi.restoreAllMocks()
    await i18n.changeLanguage('en')
  })
  it.each([
    ['en', 'Something went wrong. Try again.'],
    ['ko', '문제가 발생했습니다. 다시 시도하세요.'],
    ['ja', '問題が発生しました。もう一度お試しください。'],
    ['zh-CN', '出现了问题。请重试。'],
    ['es', 'Algo salió mal. Inténtelo de nuevo.'],
  ])('localizes actual export failure and keeps the seven-day CSV retry/download in %s', async (language, generic) => {
    await i18n.changeLanguage(language)
    const blob = new Blob(['column\nvalue'], { type: 'text/csv' })
    vi.mocked(api.exportData)
      .mockReset()
      .mockRejectedValueOnce({ code: 'future.export', message: 'export diagnostic' })
      .mockResolvedValueOnce(blob)
    vi.mocked(api.downloadBlob).mockClear()
    renderTab(<ActualExportTab />)
    fireEvent.click(screen.getByDisplayValue('csv'))
    const exportButtons = screen.getAllByRole('button').filter((button) => button.className.includes('flex-col'))
    fireEvent.click(exportButtons[0])
    const alert = await screen.findByRole('alert')
    const detail = within(alert).getByTestId('support-details')
    expect(detail.previousElementSibling).toHaveTextContent(generic)
    expect(detail.previousElementSibling).not.toHaveTextContent('export diagnostic')
    expect(detail).not.toHaveAttribute('open')
    expect(within(detail).getByTestId('support-details-text')).toHaveTextContent('future.export: export diagnostic')
    expect(api.exportData).toHaveBeenCalledTimes(1)
    expect(api.downloadBlob).not.toHaveBeenCalled()
    const [type, format, from, to] = vi.mocked(api.exportData).mock.calls[0]
    expect([type, format]).toEqual(['metrics', 'csv'])
    expect(new Date(to).getTime() - new Date(from).getTime()).toBeGreaterThanOrEqual(7 * 24 * 60 * 60 * 1000 - 10)
    expect(new Date(to).getTime() - new Date(from).getTime()).toBeLessThanOrEqual(7 * 24 * 60 * 60 * 1000)
    fireEvent.click(within(alert).getByRole('button', { name: i18n.t('settings.exportRetry') }))
    await waitFor(() => expect(api.downloadBlob).toHaveBeenCalledTimes(1))
    expect(api.exportData).toHaveBeenCalledTimes(2)
    expect(api.downloadBlob).toHaveBeenCalledWith(blob, expect.stringMatching(/^metrics_\d{4}-\d{2}-\d{2}\.csv$/))
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('retains storage classification and limits an already-unlocked reauth retry to one', async () => {
    vi.mocked(api.exportData)
      .mockReset()
      .mockRejectedValueOnce(new api.ApiClientError('auth.reauth_required', 'locked', 401))
      .mockRejectedValue(new api.ApiClientError('storage.failed', 'storage diagnostic', 500))
    vi.mocked(api.downloadBlob).mockClear()
    vi.mocked(getCaptureReauthStatus).mockResolvedValue({
      enabled: true,
      authenticated: true,
      idle_timeout_secs: 300,
      biometric_available: false,
      biometric_kind: null,
      pin_enrolled: true,
    })
    renderTab(<ActualExportTab />)
    const exportButtons = screen.getAllByRole('button').filter((button) => button.className.includes('flex-col'))
    fireEvent.click(exportButtons[2])
    const alert = await screen.findByRole('alert')
    expect(getCaptureReauthStatus).toHaveBeenCalledTimes(1)
    expect(api.exportData).toHaveBeenCalledTimes(2)
    expect(alert).toHaveTextContent(i18n.t('settings.exportStorageRecovery'))
    expect(within(alert).getByTestId('support-details-text')).toHaveTextContent('storage.failed: storage diagnostic')
    expect(api.downloadBlob).not.toHaveBeenCalled()
  })
})
