import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type RenderResult, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { ReactElement } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import DataStorageTab from './DataStorageTab'

// The tab hosts TagManagementCard, which reads the shared ['tags'] query.
// Keep the real component mounted (a missing provider here would mirror a real
// regression) but serve it an empty tag list instead of a network call.
vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  fetchTags: vi.fn().mockResolvedValue([]),
}))

function renderTab(ui: ReactElement): RenderResult {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>{ui}</MemoryRouter>
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

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
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

    await user.click(screen.getByRole('button', { name: /settings.exportFramesLabel/ }))
    expect(handleExport).toHaveBeenCalledWith('frames')
    expect(screen.getByRole('dialog', { name: 'reauth.title' })).toBeInTheDocument()
    expect(screen.getByLabelText('reauth.pinLabel')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'reauth.cancel' }))
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
          detail: 'storage.failed',
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
    expect(alert).toHaveTextContent('settings.exportFailed')
    expect(alert).toHaveTextContent('storage.failed')
    expect(alert).toHaveTextContent('settings.exportStorageRecovery')
    expect(alert).not.toHaveTextContent('settings.exportDone')

    await user.click(screen.getByRole('button', { name: 'settings.exportRetry' }))
    expect(handleExport).toHaveBeenCalledWith('metrics')
  })
})
