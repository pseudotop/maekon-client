import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import type { FeatureCapabilitySnapshot, ProviderSurfaceSpec } from '../../api/contracts'
import AdvancedTab from './AdvancedTab'
import AudioTab from './AudioTab'
import AiAutomationTab from './ai-automation'
import DataStorageTab from './DataStorageTab'
import MonitoringTab from './MonitoringTab'
import NotificationSettings from './NotificationSettings'
import PrivacySettings from './PrivacySettings'
import SyncTab from './SyncTab'
import { makeDefaultFormData } from './stories-utils'

const mockUseSettingsFormContext = vi.hoisted(() => vi.fn())
const mockUseLoadedFormData = vi.hoisted(() => vi.fn())
const mockInvoke = vi.hoisted(() => vi.fn())

vi.mock('../settings/SettingsFormContext', () => ({
  useSettingsFormContext: mockUseSettingsFormContext,
  useLoadedFormData: mockUseLoadedFormData,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

function mockSettingsContext() {
  const formData = makeDefaultFormData()
  mockUseLoadedFormData.mockReturnValue(formData)
  mockUseSettingsFormContext.mockReturnValue({
    form: {
      formData,
      exportFormat: 'json',
      exportLoading: null,
      handleExport: vi.fn(),
      handleNotificationChange: vi.fn(),
      handlePrivacyChange: vi.fn(),
      handleRootChange: vi.fn(),
      handleMonitorChange: vi.fn(),
      handleTelemetryChange: vi.fn(),
      requestNotificationPermissionMutation: { isPending: false, mutate: vi.fn() },
      requestScreenCapturePermissionMutation: { isPending: false, mutate: vi.fn() },
      testNotificationMutation: { isPending: false, mutate: vi.fn() },
      openDesktopPermissionSettingsMutation: {
        isPending: false,
        mutate: vi.fn(),
        pendingPermissionKind: null,
      },
      setExportFormat: vi.fn(),
      setFormData: vi.fn(),
    },
    data: {
      canQueryDesktopCapabilities: false,
      desktopPermissionStatus: null,
      desktopPermissionStatusError: null,
      desktopPermissionStatusLoading: false,
      desktopPermissionStatusRefreshing: false,
      handleRefreshDesktopPermissionStatus: vi.fn(),
      storageLoading: false,
      storageStats: null,
    },
  })
  return formData
}

function cliSurface(): ProviderSurfaceSpec {
  return {
    surface_id: 'provider_surface.openai.subprocess_cli',
    vendor_id: 'openai',
    display_name: 'Codex CLI',
    credential_kind: 'provider_subscription',
    execution_kind: 'subprocess_cli',
    placement_kind: 'installed_cli',
    stability: 'preview',
    preferred_for_product_auth: true,
    provisioning: null,
    llm_transport: null,
    ocr_transport: null,
    availability_probe: null,
    llm_capabilities: null,
    ocr_capabilities: null,
    default_models: {
      llm_models: [],
      ocr_models: [],
    },
    model_catalog_transport: null,
    parameter_profiles: {
      llm: { supported: [] },
      ocr: { supported: [] },
    },
    known_models: [],
    unknown_model_policy: null,
  } as unknown as ProviderSurfaceSpec
}

function featureSnapshotForCli(
  readiness: NonNullable<FeatureCapabilitySnapshot['features'][number]['provider_cli_readiness']>,
  availability: FeatureCapabilitySnapshot['features'][number]['availability'],
  dependencyStatus: FeatureCapabilitySnapshot['features'][number]['provider_cli_discovery'] extends infer Discovery
    ? Discovery extends { dependency_status: infer Status }
      ? Status | null
      : never
    : never,
): FeatureCapabilitySnapshot {
  return {
    features: [
      {
        feature_id: 'provider_surface.openai.subprocess_cli',
        maturity: 'beta',
        availability,
        provider_cli_readiness: readiness,
        provider_cli_discovery: dependencyStatus
          ? {
              candidate_name: 'codex',
              executable_path: 'C:/Tools/Codex/codex.exe',
              version_status: 'not_checked',
              dependency_status: dependencyStatus,
              status_reason: dependencyStatus === 'missing' ? 'cli_dependency_missing' : null,
              env_refresh_required: dependencyStatus === 'stale_process_env',
            }
          : null,
        preferred: true,
        requires: ['cli:codex'],
        status_reason: readiness === 'not_detected' ? 'cli_not_installed' : 'cli_detected',
        status_copy_key: 'featureCapability.surface.provider_surface.openai.subprocess_cli.partially_available',
        setup_copy_key: null,
        setup_docs_url: null,
        configuration_env_vars: [],
      },
    ],
  }
}

function mockAiAutomationContext(options?: {
  surface?: ProviderSurfaceSpec
  featureCapabilities?: FeatureCapabilitySnapshot
}) {
  const formData = makeDefaultFormData()
  formData.ai_provider = {
    ...formData.ai_provider,
    access_mode: options?.surface ? 'ProviderSubscriptionCli' : 'ProviderApiKey',
    ocr_provider: 'Local',
    llm_provider: options?.surface ? 'Remote' : 'Local',
    external_data_policy: 'PiiFilterStandard',
    llm_api: options?.surface
      ? {
          surface_id: options.surface.surface_id,
          provider: 'openai',
          model: 'gpt-5-codex',
          endpoint: null,
          api_key_masked: '',
          has_secret: false,
          backend_kind: 'unavailable',
          auth_mode: 'provider_subscription_cli',
          secret_display_hint: null,
          model_source: 'manual',
        }
      : formData.ai_provider.llm_api,
  }
  mockUseLoadedFormData.mockReturnValue(formData)
  mockUseSettingsFormContext.mockReturnValue({
    form: {
      formData,
      modelCatalogLoading: null,
      modelCatalogNotice: { ocr_api: null, llm_api: null },
      canDiscoverModels: vi.fn(() => false),
      discoverModels: vi.fn(),
      getCompatibleSurfaceOptions: vi.fn((endpointKind: string) =>
        endpointKind === 'llm_api' && options?.surface ? [options.surface] : [],
      ),
      getModelCompatibilityNotice: vi.fn(() => null),
      getModelOptions: vi.fn(() => []),
      handleAiProviderChange: vi.fn(),
      handleAutomationChange: vi.fn(),
      handleExternalApiChange: vi.fn(),
      handleOcrValidationChange: vi.fn(),
      handleProviderSurfaceChange: vi.fn(),
      handleSandboxChange: vi.fn(),
      handleSaveAiProviderProfile: vi.fn(),
      handleSceneActionOverrideChange: vi.fn(),
      handleSceneIntelligenceChange: vi.fn(),
      handleSelectAiProviderProfile: vi.fn(),
      handleDeleteAiProviderProfile: vi.fn(),
      resolveEndpointSurface: vi.fn((endpointKind: string) =>
        endpointKind === 'llm_api' ? options?.surface : undefined,
      ),
    },
    data: {
      featureCapabilities: options?.featureCapabilities ?? null,
      llmEndpointProbe: null,
      llmEndpointProbeLoading: false,
      ocrEndpointProbe: null,
      ocrEndpointProbeLoading: false,
      providerCatalog: { surfaces: options?.surface ? [options.surface] : [] },
      secretBackendCapabilities: null,
    },
  })
}

describe('Settings guidance copy', () => {
  beforeEach(() => {
    mockUseSettingsFormContext.mockReset()
    mockUseLoadedFormData.mockReset()
    mockInvoke.mockReset()
  })

  it('orients the data storage page around review, export, and retention decisions', () => {
    mockSettingsContext()

    renderWithProviders(<DataStorageTab />)

    expect(screen.getByRole('region', { name: 'Data & storage guide' })).toBeInTheDocument()
    expect(screen.getByText('Export before reducing retention')).toBeInTheDocument()
    expect(screen.getByText('Telemetry is separate')).toBeInTheDocument()
  })

  it('orients monitoring controls around permissions, intervals, and privacy mode', () => {
    mockSettingsContext()

    renderWithProviders(<MonitoringTab />)

    expect(screen.getByRole('region', { name: 'Monitoring guide' })).toBeInTheDocument()
    expect(screen.getByText('Resolve desktop access first')).toBeInTheDocument()
    expect(screen.getByText('Use privacy mode for pauses')).toBeInTheDocument()
  })

  it('orients privacy controls before users edit app and title exclusions', () => {
    const formData = makeDefaultFormData()

    renderWithProviders(<PrivacySettings privacy={formData.privacy} onChange={vi.fn()} />)

    expect(screen.getByRole('region', { name: 'Privacy guide' })).toBeInTheDocument()
    expect(screen.getByText('Start with automatic exclusions')).toBeInTheDocument()
    expect(screen.getByText('Use title patterns for sensitive workflows')).toBeInTheDocument()
  })

  it('orients notification thresholds around permission state and interruption cost', () => {
    const formData = makeDefaultFormData()

    renderWithProviders(<NotificationSettings notification={formData.notification} onChange={vi.fn()} />)

    expect(screen.getByRole('region', { name: 'Notification guide' })).toBeInTheDocument()
    expect(screen.getByText('Confirm OS permission first')).toBeInTheDocument()
    expect(screen.getByText('Keep high-usage alerts rare')).toBeInTheDocument()
  })

  it('orients audio setup around provider choice, input mode, and bystander consent', () => {
    mockSettingsContext()

    renderWithProviders(<AudioTab />)

    expect(screen.getByRole('region', { name: 'Audio setup guide' })).toBeInTheDocument()
    expect(screen.getByText('Choose local or cloud STT')).toBeInTheDocument()
    expect(screen.getByText('Pick an input mode')).toBeInTheDocument()
    expect(screen.getByText('Inform people before recording')).toBeInTheDocument()
    expect(
      screen.getByText(
        'Tell nearby participants what will be captured and obtain consent where required before enabling audio.',
      ),
    ).toBeInTheDocument()
  })

  // #7600: COMPILE-capability gate — audio is compiled OUT of the shipped
  // `grpc,windows-sandbox` release build, so the UI must disable the enable
  // toggle + downloads instead of offering a doomed download.
  it('#7600 disables audio controls and shows a not-available notice when audio_compiled=false', () => {
    const formData = makeDefaultFormData()
    mockUseLoadedFormData.mockReturnValue(formData)
    mockUseSettingsFormContext.mockReturnValue({
      form: { formData, setFormData: vi.fn() },
      data: { featureCapabilities: { features: [], audio_compiled: false } },
    })

    renderWithProviders(<AudioTab />)

    expect(screen.getByText('Not available in this build')).toBeInTheDocument()
    expect(screen.getByLabelText('Enable audio capture and STT')).toBeDisabled()
  })

  // Positive control (anti-vacuous): when audio_compiled=true the enable toggle
  // stays interactive and the not-available notice does not render.
  it('#7600 keeps audio controls enabled when audio_compiled=true', () => {
    const formData = makeDefaultFormData()
    mockUseLoadedFormData.mockReturnValue(formData)
    mockUseSettingsFormContext.mockReturnValue({
      form: { formData, setFormData: vi.fn() },
      data: { featureCapabilities: { features: [], audio_compiled: true } },
    })

    renderWithProviders(<AudioTab />)

    expect(screen.queryByText('Not available in this build')).not.toBeInTheDocument()
    expect(screen.getByLabelText('Enable audio capture and STT')).not.toBeDisabled()
  })

  it('orients advanced settings around runtime, network, and sync impact', () => {
    mockSettingsContext()

    renderWithProviders(<AdvancedTab />)

    expect(screen.getByRole('region', { name: 'Advanced settings guide' })).toBeInTheDocument()
    expect(screen.getByText('Change runtime limits carefully')).toBeInTheDocument()
    expect(screen.getByText('Pair sync settings with the sync page')).toBeInTheDocument()
  })

  it('keeps the default fractional advanced settings valid for form submission', () => {
    mockSettingsContext()

    renderWithProviders(<AdvancedTab />)

    const borderOpacity = screen.getByLabelText('Border opacity (0.0 - 1.0)')
    const minConfidence = screen.getByLabelText('Min confidence (0.0 - 1.0)')

    expect(borderOpacity).toHaveAttribute('step', '0.1')
    expect(minConfidence).toHaveAttribute('step', '0.1')
    expect(borderOpacity).toBeValid()
    expect(minConfidence).toBeValid()
  })

  it('orients sync setup when sync is disabled', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_sync_status') {
        return Promise.resolve({
          enabled: false,
          runtime_available: false,
          runtime_state: 'disabled',
          unavailable_reason: null,
          device_id: 'device-1',
          device_name: 'Work Mac',
        })
      }
      return Promise.resolve([])
    })

    renderWithProviders(<SyncTab />)

    await waitFor(() => {
      expect(screen.getByRole('region', { name: 'Sync setup guide' })).toBeInTheDocument()
    })
    expect(screen.getByText('Choose a transport deliberately')).toBeInTheDocument()
    expect(screen.getByText('Protect the passphrase')).toBeInTheDocument()
  })

  it('orients AI automation controls around access path, safeguards, and verification', () => {
    mockAiAutomationContext()

    renderWithProviders(<AiAutomationTab />)

    expect(screen.getByRole('region', { name: 'AI automation guide' })).toBeInTheDocument()
    expect(screen.getByText('Pick the access path first')).toBeInTheDocument()
    expect(screen.getByText('Keep safety gates visible')).toBeInTheDocument()
    expect(screen.getByText('Verify before widening access')).toBeInTheDocument()
    expect(
      screen.getByText('This choice controls which provider setup fields become active below.'),
    ).toBeInTheDocument()
  })

  it('renders LocalModel as the fourth access mode option in the dropdown', () => {
    mockAiAutomationContext()

    renderWithProviders(<AiAutomationTab />)

    const select = screen.getByTestId('settings-ai-access-mode')
    const options = Array.from(select.querySelectorAll('option')).map((o) => o.value)

    expect(options).toContain('ProviderApiKey')
    expect(options).toContain('ProviderSubscriptionCli')
    expect(options).toContain('ProviderOAuth')
    expect(options).toContain('LocalModel')
    expect(options[3]).toBe('LocalModel')
    expect(screen.getByRole('option', { name: 'Local model' })).toBeInTheDocument()
  })

  it('calls handleAiProviderChange with LocalModel when that option is selected', () => {
    const handleAiProviderChange = vi.fn()
    const formData = makeDefaultFormData()
    formData.ai_provider = { ...formData.ai_provider, access_mode: 'ProviderApiKey' }
    mockUseLoadedFormData.mockReturnValue(formData)
    mockUseSettingsFormContext.mockReturnValue({
      form: {
        formData,
        modelCatalogLoading: null,
        modelCatalogNotice: { ocr_api: null, llm_api: null },
        canDiscoverModels: vi.fn(() => false),
        discoverModels: vi.fn(),
        getCompatibleSurfaceOptions: vi.fn(() => []),
        getModelCompatibilityNotice: vi.fn(() => null),
        getModelOptions: vi.fn(() => []),
        handleAiProviderChange,
        handleAutomationChange: vi.fn(),
        handleExternalApiChange: vi.fn(),
        handleOcrValidationChange: vi.fn(),
        handleProviderSurfaceChange: vi.fn(),
        handleSandboxChange: vi.fn(),
        handleSaveAiProviderProfile: vi.fn(),
        handleSceneActionOverrideChange: vi.fn(),
        handleSceneIntelligenceChange: vi.fn(),
        handleSelectAiProviderProfile: vi.fn(),
        handleDeleteAiProviderProfile: vi.fn(),
        resolveEndpointSurface: vi.fn(() => undefined),
      },
      data: {
        featureCapabilities: null,
        llmEndpointProbe: null,
        llmEndpointProbeLoading: false,
        ocrEndpointProbe: null,
        ocrEndpointProbeLoading: false,
        providerCatalog: { surfaces: [] },
        secretBackendCapabilities: null,
      },
    })

    // Verify the onChange trigger calls handleAiProviderChange('access_mode', 'LocalModel')
    const { getByTestId } = renderWithProviders(<AiAutomationTab />)
    const select = getByTestId('settings-ai-access-mode') as HTMLSelectElement

    fireEvent.change(select, { target: { value: 'LocalModel' } })

    expect(handleAiProviderChange).toHaveBeenCalledWith('access_mode', 'LocalModel')
  })

  it('shows provider CLI readiness and dependency states in the LLM surface panel', () => {
    const surface = cliSurface()
    const scenarios = [
      {
        readiness: 'auth_required' as const,
        availability: 'partially_available' as const,
        dependencyStatus: 'ready' as const,
        expected: ['Sign-in required', 'Dependency ready', 'Partially available'],
      },
      {
        readiness: 'auth_ready' as const,
        availability: 'partially_available' as const,
        dependencyStatus: 'ready' as const,
        expected: ['Signed in; invocation unavailable', 'Dependency ready'],
      },
      {
        readiness: 'auth_unverified' as const,
        availability: 'partially_available' as const,
        dependencyStatus: 'missing' as const,
        expected: ['Auth not verified', 'Dependency missing'],
      },
      {
        readiness: 'not_detected' as const,
        availability: 'unavailable' as const,
        dependencyStatus: null,
        expected: ['CLI not detected', 'Unavailable'],
      },
      {
        readiness: 'invocation_ready' as const,
        availability: 'partially_available' as const,
        dependencyStatus: 'stale_process_env' as const,
        expected: ['Ready to invoke', 'Restart required'],
      },
    ]

    for (const scenario of scenarios) {
      cleanup()
      mockAiAutomationContext({
        surface,
        featureCapabilities: featureSnapshotForCli(
          scenario.readiness,
          scenario.availability,
          scenario.dependencyStatus,
        ),
      })

      renderWithProviders(<AiAutomationTab />)

      for (const label of scenario.expected) {
        expect(screen.getAllByText(label).length).toBeGreaterThan(0)
      }
    }
  })

  // #7678: PLATFORM-capability gate — local OCR silently returns zero regions
  // forever on a platform/build with no compiled OCR engine (e.g. every
  // shipped Linux build today), so the "Local" OCR provider selection must
  // warn instead of staying silent.
  it('#7678 warns when Local OCR is selected but no local OCR engine is available', () => {
    mockAiAutomationContext({
      featureCapabilities: { features: [], ocr_available: false },
    })

    renderWithProviders(<AiAutomationTab />)

    expect(screen.getByText('Not available in this build')).toBeInTheDocument()
  })

  // Positive control (anti-vacuous): when ocr_available=true (and OCR provider
  // stays Local, the default in mockAiAutomationContext) the warning must not render.
  it('#7678 shows no OCR warning when ocr_available=true', () => {
    mockAiAutomationContext({
      featureCapabilities: { features: [], ocr_available: true },
    })

    renderWithProviders(<AiAutomationTab />)

    expect(screen.queryByText('Not available in this build')).not.toBeInTheDocument()
  })
})
