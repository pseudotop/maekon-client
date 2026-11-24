/**
 * First-run onboarding (#13734): five steps, an AI step that connects a
 * provider inside setup, AI consents taken one at a time, and a restart the
 * step resumes after. Navigation uses test ids and `data-step`; locale values
 * are imported only to check what the user reads.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import type { AppSettings } from '../api/client'
import type {
  AiCapabilityId,
  AiCapabilityReadiness,
  AiReadinessDimensions,
  AiReadinessReasonCode,
  ChatHttpVerificationStatus,
  ConsentPermissions,
  ConsentSnapshot,
  FeatureCapabilitySnapshot,
} from '../api/contracts'
import { DEFAULT_PROVIDER_SURFACE_CATALOG } from '../api/defaultProviderSurfaceCatalog'
import { providerSurfaceById } from '../features/providerSurfaces'
import * as aiReadinessModule from '../hooks/useAiReadinessSnapshot'
import i18n from '../i18n'
import en from '../i18n/locales/en.json'
import Onboarding, { ONBOARDING_STEP_STORAGE_KEY } from './Onboarding'
import { makeDefaultFormData } from './setting-tabs/stories-utils'

// Every IPC call reaches this fake through the runtime bridge that
// `@tauri-apps/api/core` calls. A module mock is not enough here: when several
// calls import that module at once, only the first resolves to the mock.
const mockInvoke = vi.fn()
const platform = vi.hoisted(() => ({ tauri: false }))

vi.mock('../utils/platform', () => ({
  IS_LINUX: false,
  IS_MAC: true,
  get IS_TAURI() {
    return platform.tauri
  },
  IS_WINDOWS: false,
  MOD_KEY: '⌘',
  isTauriRuntime: vi.fn(() => platform.tauri),
}))

// Settings travel over HTTP; consent, readiness and the key check over IPC.
const mockFetchSettings = vi.fn()
const mockUpdateSettings = vi.fn()
vi.mock('../api/client', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../api/client')>()
  const offline = () => Promise.reject(new Error('offline in tests'))
  return {
    ...mod,
    fetchSettings: (...args: unknown[]) => mockFetchSettings(...args),
    updateSettings: (...args: unknown[]) => mockUpdateSettings(...args),
    fetchStorageStats: offline,
    fetchUpdateStatus: offline,
    fetchProviderSurfaces: offline,
  }
})

/* ── Fixtures ── */

const NO_PERMISSIONS: ConsentPermissions = {
  screen_capture: false,
  ocr_processing: false,
  telemetry: false,
  process_monitoring: false,
  input_activity: false,
  window_title_collection: false,
  app_usage_analytics: false,
  clipboard_monitoring: false,
  file_access_monitoring: false,
  activity_pattern_learning: false,
  cross_device_sync: false,
  full_text_extraction: false,
  memory_graph_enrichment: false,
  microphone: false,
  unredacted_external_ocr: false,
  memory_graph_retrieval_ranking: false,
  memory_vault_mirror: false,
}

function permissions(overrides: Partial<ConsentPermissions> = {}): ConsentPermissions {
  return { ...NO_PERMISSIONS, ...overrides }
}

// #9631: the first-run bundle includes ocr_processing; every other opt-in stays off.
const MONITORING_GRANT = permissions({
  screen_capture: true,
  ocr_processing: true,
  window_title_collection: true,
  app_usage_analytics: true,
  process_monitoring: true,
  input_activity: true,
  telemetry: true,
})

const valid = (granted: ConsentPermissions): ConsentSnapshot => ({ status: 'Valid', permissions: granted })

/** Monitoring plus the one consent a provider needs. */
const AI_CONSENT = valid({ ...MONITORING_GRANT, full_text_extraction: true })

const READY_DIMENSIONS: AiReadinessDimensions = {
  compiled_capability: true,
  selected_access_mode: 'provider_api_key',
  access_mode_compatible: true,
  endpoint_or_profile_configured: true,
  provider_detection: 'not_required',
  provider_auth: 'ready',
  provider_invocation: 'ready',
  model_availability: 'available',
  runtime_flag_enabled: true,
  consent: [{ field: 'full_text_extraction', granted: true }],
  apply_requirement: 'restart',
  apply_pending: false,
  privacy_gate: 'enforced_at_invocation',
  egress_gate: 'enforced_at_invocation',
  budget_gate: 'enforced_at_invocation',
  audit_gate: 'enforced_at_invocation',
}

function entry(
  capabilityId: AiCapabilityId,
  reason: AiReadinessReasonCode,
  dimensions: Partial<AiReadinessDimensions> = {},
): AiCapabilityReadiness {
  return {
    capability_id: capabilityId,
    status: reason === 'ready' ? 'ready' : 'blocked',
    reason_code: reason,
    action: 'none',
    action_copy_key: 'aiReadiness.action.none',
    dimensions: { ...READY_DIMENSIONS, ...dimensions },
  }
}

function capabilities(...entries: AiCapabilityReadiness[]): FeatureCapabilitySnapshot {
  return { features: [], ai_readiness: { contract_version: 1, capabilities: entries } }
}

/** First run: no provider saved. */
const NO_PROVIDER = capabilities(
  entry('chat.http_api', 'endpoint_or_profile_required', {
    endpoint_or_profile_configured: false,
    provider_auth: 'required',
    provider_invocation: 'unverified',
    model_availability: 'unverified',
  }),
)

/** An API-key provider was just saved; only a restart applies it. */
const SAVED_AWAITING_RESTART = capabilities(
  entry('chat.http_api', 'provider_invocation_unverified', {
    provider_invocation: 'unverified',
    model_availability: 'unverified',
    apply_pending: true,
  }),
)

/** Reopened after the restart: one key check settles it. */
const AFTER_RESTART = capabilities(
  entry('chat.http_api', 'provider_invocation_unverified', {
    provider_invocation: 'unverified',
    model_availability: 'unverified',
  }),
)

// Summaries and suggestions also need pattern learning, which was left unchecked.
const PATTERN_LEARNING_OFF = { field: 'activity_pattern_learning', granted: false } as const
const CONNECTED = capabilities(
  entry('chat.http_api', 'ready'),
  entry('ocr.suggestion_analysis', 'consent_required', {
    consent: [
      { field: 'ocr_processing', granted: true },
      PATTERN_LEARNING_OFF,
      { field: 'full_text_extraction', granted: true },
    ],
  }),
  entry('segment_summary', 'consent_required', {
    consent: [PATTERN_LEARNING_OFF, { field: 'full_text_extraction', granted: true }],
  }),
  entry('daily_narrative', 'consent_required', {
    consent: [PATTERN_LEARNING_OFF, { field: 'full_text_extraction', granted: true }],
  }),
)

/** A first run: no provider, analysis off (#8059 G2b) and coaching off (#5707). */
function firstRunSettings(): AppSettings {
  const base = makeDefaultFormData()
  return {
    ...base,
    analysis: {
      ...base.analysis,
      enabled: false,
      tiered_memory_enabled: false,
      embedding_enabled: false,
      llm_summary_enabled: false,
    },
    coaching: { ...base.coaching, enabled: false },
  }
}

/* ── Fake backend ── */

interface Backend {
  /** null: there is no consent record to read. */
  consent: ConsentSnapshot | null
  /** null: readiness cannot be read. */
  readiness: FeatureCapabilitySnapshot | null
  settings: AppSettings
  verify: ChatHttpVerificationStatus | 'reject'
  consentWriteError: unknown
  /** null: the quit request goes through. */
  quitError: unknown
  /** What readiness reports once a settings save lands. */
  readinessAfterSave: FeatureCapabilitySnapshot | null
  /** What readiness reports once a key check succeeds. */
  readinessAfterVerify: FeatureCapabilitySnapshot | null
}

const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T

function installBackend(overrides: Partial<Backend> = {}): Backend {
  const backend: Backend = {
    consent: { status: 'NotGranted', permissions: permissions() },
    readiness: null,
    settings: firstRunSettings(),
    verify: 'verified',
    consentWriteError: null,
    quitError: null,
    readinessAfterSave: null,
    readinessAfterVerify: null,
    ...overrides,
  }
  mockInvoke.mockImplementation((cmd: string, args?: { permissions?: ConsentPermissions }) => {
    switch (cmd) {
      case 'get_consent':
        return backend.consent ? Promise.resolve(clone(backend.consent)) : Promise.reject(new Error('no consent store'))
      case 'set_consent':
        if (backend.consentWriteError) return Promise.reject(backend.consentWriteError)
        backend.consent = valid(clone(args?.permissions ?? NO_PERMISSIONS))
        return Promise.resolve(clone(backend.consent))
      case 'get_feature_capabilities':
        return backend.readiness ? Promise.resolve(clone(backend.readiness)) : Promise.reject(new Error('no readiness'))
      case 'verify_chat_http_provider':
        if (backend.verify === 'reject') {
          return Promise.reject({ code: 'service.unavailable', message: 'session manager not available' })
        }
        if (backend.verify === 'verified' && backend.readinessAfterVerify)
          backend.readiness = backend.readinessAfterVerify
        return Promise.resolve({ status: backend.verify })
      case 'request_app_quit':
        return backend.quitError ? Promise.reject(backend.quitError) : Promise.resolve(undefined)
      case 'complete_onboarding':
        return Promise.resolve(undefined)
      default:
        return Promise.reject(new Error(`${cmd} is not part of this test`))
    }
  })
  mockFetchSettings.mockImplementation(() => Promise.resolve(clone(backend.settings)))
  mockUpdateSettings.mockImplementation((next: AppSettings) => {
    backend.settings = clone(next)
    if (backend.readinessAfterSave) backend.readiness = backend.readinessAfterSave
    return Promise.resolve(clone(next))
  })
  return backend
}

/* ── Helpers ── */

const card = () => screen.getByTestId('onboarding-card')
const providerSection = () => screen.getByTestId('onboarding-ai-provider')

async function expectStep(step: number) {
  await waitFor(() => expect(card()).toHaveAttribute('data-step', String(step)))
}

async function expectAiPhase(phase: string) {
  await waitFor(() => expect(providerSection()).toHaveAttribute('data-phase', phase))
}

function startAt(step: number) {
  window.localStorage.setItem(ONBOARDING_STEP_STORAGE_KEY, String(step))
}

function renderOnboarding(onComplete = vi.fn()) {
  const view = renderWithProviders(<Onboarding onComplete={onComplete} />)
  return { onComplete, unmount: view.unmount }
}

function consentWrites(): ConsentPermissions[] {
  return mockInvoke.mock.calls
    .filter(([cmd]) => cmd === 'set_consent')
    .map(([, args]) => (args as { permissions: ConsentPermissions }).permissions)
}

function invokedCommands(): string[] {
  return mockInvoke.mock.calls.map(([cmd]) => String(cmd))
}

function saveAnthropic(apiKey: string) {
  fireEvent.click(screen.getByTestId('provider-wizard-option-anthropic'))
  expect(screen.getByTestId('provider-wizard-save')).toBeDisabled()
  fireEvent.change(screen.getByTestId('provider-wizard-api-key'), { target: { value: apiKey } })
  fireEvent.click(screen.getByTestId('provider-wizard-save'))
}

async function waitForConsentControls() {
  await waitFor(() => expect(screen.getByTestId('onboarding-ai-consent-full-text')).toBeEnabled())
}

describe('Onboarding', () => {
  beforeEach(() => {
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (cmd: string, args?: unknown) => mockInvoke(cmd, args),
    }
    mockInvoke.mockReset()
    mockFetchSettings.mockReset()
    mockUpdateSettings.mockReset()
    window.localStorage.clear()
    aiReadinessModule.invalidateAiReadinessSnapshotCache()
  })

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__
    vi.restoreAllMocks()
  })

  describe('steps', () => {
    it('walks five steps without AI or monitoring and records completion', async () => {
      installBackend()
      const { onComplete } = renderOnboarding()

      expect(card().querySelectorAll('fieldset > div')).toHaveLength(5)
      expect(card()).toHaveAttribute('data-step', '0')
      fireEvent.click(screen.getByTestId('onboarding-next'))
      await expectStep(1)
      fireEvent.click(screen.getByTestId('onboarding-next'))
      await expectStep(2)
      fireEvent.click(screen.getByTestId('onboarding-consent-decline'))
      await expectStep(3)
      await expectAiPhase('consent')
      expect(screen.getByTestId('onboarding-next')).toBeDisabled()
      fireEvent.click(screen.getByTestId('onboarding-ai-skip'))
      await expectStep(4)
      fireEvent.click(screen.getByTestId('onboarding-complete'))

      await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1))
      expect(invokedCommands()).toContain('complete_onboarding')
      expect(window.localStorage.getItem(ONBOARDING_STEP_STORAGE_KEY)).toBeNull()
      expect(consentWrites()).toEqual([])
      expect(mockUpdateSettings).not.toHaveBeenCalled()
    })

    it('explains what Maekon does before it asks for anything', () => {
      installBackend()
      renderOnboarding()

      for (const key of ['capture', 'resume', 'search'] as const) {
        expect(screen.getByText(en.onboarding.intro[`${key}Title`])).toBeInTheDocument()
        expect(screen.getByText(en.onboarding.intro[`${key}Desc`])).toBeInTheDocument()
      }
      expect(screen.getByText(en.onboarding.intro.privacyNote)).toBeInTheDocument()
      expect(consentWrites()).toEqual([])
    })

    it('ends setup from the intro when the user skips it', async () => {
      installBackend()
      const { onComplete } = renderOnboarding()

      fireEvent.click(screen.getByTestId('onboarding-skip'))

      await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1))
      expect(invokedCommands()).toContain('complete_onboarding')
    })

    it('separates required macOS permissions from recommended notifications', async () => {
      installBackend()
      renderOnboarding()

      fireEvent.click(screen.getByTestId('onboarding-next'))

      await expectStep(1)
      expect(screen.getByText(en.onboarding.step2DescMac)).toBeInTheDocument()
    })

    it('stores the step so setup resumes after a restart, and clears it at the start', async () => {
      installBackend()
      renderOnboarding()

      fireEvent.click(screen.getByTestId('onboarding-next'))
      await expectStep(1)
      expect(window.localStorage.getItem(ONBOARDING_STEP_STORAGE_KEY)).toBe('1')

      fireEvent.click(screen.getByTestId('onboarding-back'))
      await expectStep(0)
      expect(window.localStorage.getItem(ONBOARDING_STEP_STORAGE_KEY)).toBeNull()
    })

    it('resumes at a stored step and starts over for anything else', () => {
      installBackend()
      for (const stored of ['5', '-1', '2.5', 'abc', '']) {
        window.localStorage.setItem(ONBOARDING_STEP_STORAGE_KEY, stored)
        const { unmount } = renderOnboarding()
        expect(card(), `stored ${JSON.stringify(stored)}`).toHaveAttribute('data-step', '0')
        unmount()
      }

      startAt(4)
      renderOnboarding()
      expect(card()).toHaveAttribute('data-step', '4')
    })
  })

  describe('monitoring consent', () => {
    it('lists what is collected and grants the seven-field bundle only on the click', async () => {
      const invalidate = vi.spyOn(aiReadinessModule, 'invalidateAiReadinessSnapshotCache')
      installBackend()
      startAt(2)
      renderOnboarding()

      for (const item of ['screenFrames', 'ocrText', 'guiInteractions'] as const) {
        expect(screen.getByText(en.privacy.consent.monitoring.collected[item])).toBeInTheDocument()
      }
      expect(consentWrites()).toEqual([])

      fireEvent.click(screen.getByTestId('onboarding-consent-grant'))

      await expectStep(3)
      expect(consentWrites()).toEqual([MONITORING_GRANT])
      expect(invalidate).toHaveBeenCalled()
    })

    it('keeps AI opt-ins already in force when it grants', async () => {
      installBackend({ consent: valid(permissions({ full_text_extraction: true })) })
      startAt(2)
      renderOnboarding()

      fireEvent.click(screen.getByTestId('onboarding-consent-grant'))

      await expectStep(3)
      expect(consentWrites()).toEqual([{ ...MONITORING_GRANT, full_text_extraction: true }])
    })

    it('carries nothing over from an expired record', async () => {
      installBackend({
        consent: { status: 'Expired', permissions: permissions({ full_text_extraction: true, microphone: true }) },
      })
      startAt(2)
      renderOnboarding()

      fireEvent.click(screen.getByTestId('onboarding-consent-grant'))

      await expectStep(3)
      expect(consentWrites()).toEqual([MONITORING_GRANT])
    })

    it('continues without monitoring, says what that means, and writes nothing', async () => {
      installBackend()
      startAt(2)
      renderOnboarding()

      expect(screen.getByTestId('onboarding-consent-decline-note')).toHaveTextContent(
        en.privacy.consent.monitoring.label,
      )
      fireEvent.click(screen.getByTestId('onboarding-consent-decline'))

      await expectStep(3)
      expect(consentWrites()).toEqual([])
    })

    it('stays on the step with a readable reason when the grant fails', async () => {
      installBackend({ consentWriteError: new Error('consent store is locked') })
      startAt(2)
      renderOnboarding()

      fireEvent.click(screen.getByTestId('onboarding-consent-grant'))

      expect(await screen.findByText('consent store is locked')).toBeInTheDocument()
      expect(card()).toHaveAttribute('data-step', '2')
      expect(screen.getByTestId('onboarding-consent-grant')).toBeEnabled()
    })

    it('shows the grant in force to someone who comes back', async () => {
      installBackend({ consent: valid(MONITORING_GRANT) })
      startAt(2)
      renderOnboarding()

      expect(await screen.findByTestId('onboarding-consent-granted')).toBeInTheDocument()
      expect(screen.queryByTestId('onboarding-consent-grant')).toBeNull()
      fireEvent.click(screen.getByTestId('onboarding-next'))

      await expectStep(3)
      expect(consentWrites()).toEqual([])
    })
  })

  describe('AI consents', () => {
    it('saves each AI consent on its own and never one left unchecked', async () => {
      installBackend({ consent: valid(MONITORING_GRANT) })
      startAt(3)
      renderOnboarding()

      await waitForConsentControls()
      expect(screen.getByTestId('onboarding-ai-consent-full-text')).not.toBeChecked()
      expect(screen.getByTestId('onboarding-ai-consent-ocr')).toBeChecked()
      expect(screen.getByTestId('onboarding-ai-consent-pattern-learning')).not.toBeChecked()

      fireEvent.click(screen.getByTestId('onboarding-ai-consent-full-text'))
      await waitFor(() => expect(screen.getByTestId('onboarding-ai-consent-full-text')).toBeChecked())
      await waitForConsentControls()
      fireEvent.click(screen.getByTestId('onboarding-ai-consent-ocr'))
      await waitFor(() => expect(screen.getByTestId('onboarding-ai-consent-ocr')).not.toBeChecked())

      expect(consentWrites()).toEqual([
        { ...MONITORING_GRANT, full_text_extraction: true },
        { ...MONITORING_GRANT, full_text_extraction: true, ocr_processing: false },
      ])
      expect(screen.getByTestId('onboarding-ai-consent-pattern-learning')).not.toBeChecked()
      // A consent is not a setting: coaching (#5707) and the analysis flags stay as they were.
      expect(mockUpdateSettings).not.toHaveBeenCalled()
    })

    it('reports a failed consent write next to its row and shows the record as it is', async () => {
      installBackend({ consent: valid(MONITORING_GRANT), consentWriteError: new Error('consent store is locked') })
      startAt(3)
      renderOnboarding()

      await waitForConsentControls()
      fireEvent.click(screen.getByTestId('onboarding-ai-consent-full-text'))

      expect(await screen.findByText('consent store is locked')).toBeInTheDocument()
      await waitForConsentControls()
      expect(screen.getByTestId('onboarding-ai-consent-full-text')).not.toBeChecked()
      expect(providerSection()).toHaveAttribute('data-phase', 'consent')
    })

    it('asks for the full-text consent before it offers a provider', async () => {
      installBackend({ consent: valid(MONITORING_GRANT) })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('consent')
      expect(screen.getByTestId('onboarding-ai-needs-consent')).toHaveTextContent(en.privacy.consent.fullText.label)
      expect(screen.queryByTestId('provider-wizard-option-anthropic')).toBeNull()
      expect(screen.getByTestId('onboarding-next')).toBeDisabled()
      expect(screen.getByTestId('onboarding-ai-without-note')).toHaveTextContent(en.onboarding.ai.withoutNote)
    })
  })

  describe('AI provider', () => {
    it('connects a provider across the restart and finishes with AI on', async () => {
      const backend = installBackend({
        consent: AI_CONSENT,
        readiness: NO_PROVIDER,
        readinessAfterSave: SAVED_AWAITING_RESTART,
        readinessAfterVerify: CONNECTED,
      })
      startAt(3)
      const beforeRestart = renderOnboarding()

      await expectAiPhase('provider')
      saveAnthropic('sk-ant-test')

      await waitFor(() => expect(mockUpdateSettings).toHaveBeenCalledTimes(1))
      const saved = mockUpdateSettings.mock.calls[0][0] as AppSettings
      expect(saved.ai_provider).toMatchObject({ access_mode: 'ProviderApiKey', llm_provider: 'Remote' })
      expect(saved.ai_provider.llm_api).toMatchObject({
        surface_id: 'provider_surface.anthropic.direct_api',
        api_key_masked: 'sk-ant-test',
        model: 'claude-sonnet-5',
      })
      // #8059 G2b: this save is the explicit opt-in the hint above the wizard discloses.
      expect(saved.analysis).toMatchObject({
        enabled: true,
        tiered_memory_enabled: true,
        embedding_enabled: true,
        llm_summary_enabled: true,
      })
      // #5707: coaching stays opt-in in Settings.
      expect(saved.coaching.enabled).toBe(false)

      // Checking before the restart only returns restart_required, so the step offers neither a check nor Next.
      await expectAiPhase('restart')
      const surface = providerSurfaceById(DEFAULT_PROVIDER_SURFACE_CATALOG, 'provider_surface.anthropic.direct_api')
      expect(screen.getByTestId('onboarding-ai-restart')).toHaveTextContent(surface?.display_name ?? 'missing surface')
      expect(screen.queryByTestId('settings-chat-http-verify')).toBeNull()
      expect(screen.queryByTestId('onboarding-next')).toBeNull()
      fireEvent.click(screen.getByTestId('onboarding-ai-quit'))
      await waitFor(() => expect(invokedCommands()).toContain('request_app_quit'))
      expect(window.localStorage.getItem(ONBOARDING_STEP_STORAGE_KEY)).toBe('3')
      beforeRestart.unmount()

      // Maekon opens again: the provider is applied and setup resumes on this step.
      backend.readiness = AFTER_RESTART
      aiReadinessModule.invalidateAiReadinessSnapshotCache()
      const { onComplete } = renderOnboarding()

      expect(card()).toHaveAttribute('data-step', '3')
      await expectAiPhase('verify')
      expect(screen.getByTestId('onboarding-next')).toBeDisabled()
      fireEvent.click(screen.getByTestId('settings-chat-http-verify'))

      await expectAiPhase('connected')
      expect(screen.getByTestId('onboarding-next')).toBeEnabled()
      expect(screen.queryByTestId('onboarding-ai-without-note')).toBeNull()
      const features = await screen.findByTestId('onboarding-ai-features')
      for (const row of within(features).getAllByRole('listitem')) {
        expect(row).toHaveAttribute('data-status', 'needsConsent')
        expect(row).toHaveTextContent(en.privacy.consent.patternLearning.label)
      }

      fireEvent.click(screen.getByTestId('onboarding-next'))
      await expectStep(4)
      await waitFor(() => expect(screen.getByTestId('onboarding-ready-ai')).toHaveAttribute('data-status', 'connected'))
      expect(card()).not.toHaveTextContent(en.onboarding.ready.recoveryNeedsAi)
      fireEvent.click(screen.getByTestId('onboarding-complete'))
      await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1))
    })

    // The acceptance mutation for #13734: treating any finished check as a
    // connection must turn this red.
    it.each([
      ['provider_error', en.settingsAutomation.chatVerification.result.providerError],
      ['timeout', en.settingsAutomation.chatVerification.result.timeout],
      ['budget_exhausted', en.settingsAutomation.chatVerification.result.budgetExhausted],
      ['not_configured', en.settingsAutomation.chatVerification.result.notConfigured],
      ['consent_required', en.settingsAutomation.chatVerification.result.consentRequired],
      ['reject', en.settingsAutomation.chatVerification.result.failed],
    ] as const)('keeps Next off and says why when the key check ends in %s', async (verify, reason) => {
      installBackend({ consent: AI_CONSENT, readiness: AFTER_RESTART, verify, readinessAfterVerify: CONNECTED })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('verify')
      fireEvent.click(screen.getByTestId('settings-chat-http-verify'))

      const result = await screen.findByTestId('settings-chat-http-verification-result')
      expect(result).toHaveAttribute('data-status', verify === 'reject' ? 'failed' : verify)
      expect(result).toHaveTextContent(reason)
      expect(providerSection()).toHaveAttribute('data-phase', 'verify')
      expect(screen.queryByTestId('onboarding-ai-connected')).toBeNull()
      expect(screen.getByTestId('onboarding-next')).toBeDisabled()
      expect(screen.getByTestId('settings-chat-http-verify')).toBeEnabled()
    })

    it('turns a check that needs a restart into the restart card', async () => {
      installBackend({ consent: AI_CONSENT, readiness: AFTER_RESTART, verify: 'restart_required' })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('verify')
      fireEvent.click(screen.getByTestId('settings-chat-http-verify'))

      await expectAiPhase('restart')
      expect(screen.queryByTestId('onboarding-next')).toBeNull()
    })

    it('keeps the restart card and says so when Maekon cannot quit itself', async () => {
      installBackend({ consent: AI_CONSENT, readiness: SAVED_AWAITING_RESTART, quitError: new Error('tray is gone') })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('restart')
      fireEvent.click(screen.getByTestId('onboarding-ai-quit'))

      expect(await screen.findByTestId('onboarding-ai-quit-failed')).toHaveTextContent(en.onboarding.ai.quitFailed)
      expect(providerSection()).toHaveAttribute('data-phase', 'restart')
      expect(screen.getByTestId('onboarding-ai-quit')).toBeEnabled()
      // Restarting by hand still resumes setup on this step.
      expect(window.localStorage.getItem(ONBOARDING_STEP_STORAGE_KEY)).toBe('3')
    })

    it('shows why a provider could not be saved and offers the wizard again', async () => {
      installBackend({ consent: AI_CONSENT, readiness: NO_PROVIDER })
      mockUpdateSettings.mockRejectedValue(new Error('settings file is read-only'))
      startAt(3)
      renderOnboarding()

      await expectAiPhase('provider')
      saveAnthropic('sk-ant-test')

      expect(await screen.findByTestId('onboarding-ai-save-error')).toHaveTextContent('settings file is read-only')
      expect(providerSection()).toHaveAttribute('data-phase', 'provider')
      expect(screen.getByTestId('provider-wizard-option-anthropic')).toBeInTheDocument()
      expect(screen.getByTestId('onboarding-next')).toBeDisabled()
    })

    it('lets the user pick another provider while a restart is pending, or keep the saved one', async () => {
      installBackend({ consent: AI_CONSENT, readiness: SAVED_AWAITING_RESTART })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('restart')
      fireEvent.click(screen.getByTestId('onboarding-ai-change-provider'))
      await expectAiPhase('provider')
      expect(screen.getByTestId('provider-wizard-option-anthropic')).toBeInTheDocument()
      fireEvent.click(screen.getByTestId('onboarding-ai-change-cancel'))

      await expectAiPhase('restart')
      expect(mockUpdateSettings).not.toHaveBeenCalled()
    })

    it('says plainly when this build cannot use AI and offers no provider', async () => {
      installBackend({
        consent: AI_CONSENT,
        readiness: capabilities(entry('chat.http_api', 'compiled_capability_missing', { compiled_capability: false })),
      })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('blocked')
      expect(screen.getByTestId('onboarding-ai-blocked')).toHaveTextContent(en.onboarding.ai.unavailable)
      expect(screen.queryByTestId('provider-wizard-option-anthropic')).toBeNull()
      expect(screen.getByTestId('onboarding-next')).toBeDisabled()
    })

    it('offers another provider when the saved one cannot be reached', async () => {
      installBackend({
        consent: AI_CONSENT,
        readiness: capabilities(
          entry('chat.http_api', 'provider_invocation_unavailable', { provider_invocation: 'unavailable' }),
        ),
      })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('blocked')
      expect(screen.getByTestId('onboarding-ai-blocked')).toHaveTextContent(en.onboarding.ai.unreachable)
      expect(screen.getByTestId('provider-wizard-option-anthropic')).toBeInTheDocument()
      expect(screen.getByTestId('onboarding-next')).toBeDisabled()
    })

    it('shows what each AI feature still needs once chat works', async () => {
      installBackend({
        consent: valid({ ...MONITORING_GRANT, full_text_extraction: true, activity_pattern_learning: true }),
        readiness: capabilities(
          entry('chat.http_api', 'ready'),
          entry('ocr.suggestion_analysis', 'ready'),
          entry('segment_summary', 'restart_required', { apply_pending: true }),
          entry('daily_narrative', 'runtime_flag_disabled', { runtime_flag_enabled: false }),
        ),
      })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('connected')
      const rows = within(screen.getByTestId('onboarding-ai-features')).getAllByRole('listitem')
      expect(rows.map((row) => [row.dataset.capabilityId, row.dataset.status])).toEqual([
        ['ocr.suggestion_analysis', 'on'],
        ['segment_summary', 'afterRestart'],
        ['daily_narrative', 'inSettings'],
      ])
      expect(screen.getByTestId('onboarding-next')).toBeEnabled()
    })
  })

  describe('ready step', () => {
    it('finishes without AI and says what stays off', async () => {
      installBackend({ consent: valid(MONITORING_GRANT), readiness: NO_PROVIDER })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('consent')
      fireEvent.click(screen.getByTestId('onboarding-ai-skip'))

      await expectStep(4)
      const note = await screen.findByTestId('onboarding-ready-ai')
      expect(note).toHaveAttribute('data-status', 'off')
      expect(note).toHaveTextContent(en.onboarding.ready.ai.off)
      expect(card()).toHaveTextContent(en.onboarding.ready.recoveryNeedsAi)
      expect(screen.getByText(en.onboarding.ready.desc)).toBeInTheDocument()
      // Continuing without AI saves nothing: summaries stay off (#8059 G2b) and no AI consent is written.
      expect(mockUpdateSettings).not.toHaveBeenCalled()
      expect(consentWrites()).toEqual([])
    })

    it('says AI turns on after a restart when the restart is put off', async () => {
      installBackend({ consent: AI_CONSENT, readiness: SAVED_AWAITING_RESTART })
      startAt(3)
      renderOnboarding()

      await expectAiPhase('restart')
      fireEvent.click(screen.getByTestId('onboarding-ai-restart-later'))

      await expectStep(4)
      const note = await screen.findByTestId('onboarding-ready-ai')
      expect(note).toHaveAttribute('data-status', 'restartPending')
      expect(note).toHaveTextContent(en.onboarding.ready.ai.restartPending)
    })

    // #9811: the consent step does not gate Next, so the last step must not
    // claim more than the app is doing.
    it('says collection is off, not "all set", when monitoring was skipped', async () => {
      installBackend()
      startAt(4)
      renderOnboarding()

      expect(await screen.findByTestId('onboarding-ready-collection-off')).toBeInTheDocument()
      expect(screen.getByText(en.onboarding.readyCollectionOffTitle)).toBeInTheDocument()
      expect(screen.queryByText(en.onboarding.ready.desc)).toBeNull()
    })

    it('grants from the last step without dropping AI opt-ins in force', async () => {
      installBackend({ consent: valid(permissions({ full_text_extraction: true })) })
      startAt(4)
      renderOnboarding()

      const offer = await screen.findByTestId('onboarding-ready-collection-off')
      fireEvent.click(within(offer).getByRole('button'))

      // The warning clears because the re-read finds the grant, not because the button was pressed.
      await waitFor(() => expect(screen.queryByTestId('onboarding-ready-collection-off')).toBeNull())
      expect(consentWrites()).toEqual([{ ...MONITORING_GRANT, full_text_extraction: true }])
      expect(screen.getByText(en.onboarding.ready.desc)).toBeInTheDocument()
    })

    it('stays quiet when monitoring is on', async () => {
      installBackend({ consent: valid(MONITORING_GRANT) })
      startAt(4)
      renderOnboarding()

      await waitFor(() => expect(invokedCommands()).toContain('get_consent'))
      expect(await screen.findByText(en.onboarding.ready.desc)).toBeInTheDocument()
      expect(screen.queryByTestId('onboarding-ready-collection-off')).toBeNull()
    })
  })
})

describe('Onboarding localized errors at real action boundaries (#13756)', () => {
  beforeEach(() => {
    platform.tauri = false
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (cmd: string, args?: unknown) => mockInvoke(cmd, args),
    }
    mockInvoke.mockReset()
    mockFetchSettings.mockReset()
    mockUpdateSettings.mockReset()
    localStorage.clear()
    aiReadinessModule.invalidateAiReadinessSnapshotCache()
  })
  afterEach(async () => {
    platform.tauri = false
    vi.restoreAllMocks()
    await i18n.changeLanguage('en')
  })
  it.each([
    ['en', 'Could not read the current desktop permission state.'],
    ['ko', '현재 데스크톱 권한 상태를 읽지 못했습니다.'],
    ['ja', '現在のデスクトップ権限状態を読み取れませんでした。'],
    ['zh-CN', '无法读取当前桌面权限状态。'],
    ['es', 'No se pudo leer el estado actual de los permisos de escritorio.'],
  ])('shows a permission read failure and recovers through the same IPC in %s', async (language, primary) => {
    await i18n.changeLanguage(language)
    platform.tauri = true
    installBackend()
    let attempts = 0
    const bridge = mockInvoke.getMockImplementation()!
    const granted = { state: 'granted', status_reason: null }
    mockInvoke.mockImplementation((command, args) => {
      if (command === 'get_desktop_permission_status') {
        if (++attempts === 1) return Promise.reject(new Error('permission probe diagnostic'))
        return Promise.resolve({
          platform: 'macos',
          accessibility: granted,
          screen_capture: granted,
          microphone: granted,
          input_monitoring: granted,
          notifications: granted,
        })
      }
      return bridge(command, args)
    })
    startAt(1)
    renderOnboarding()
    const details = await screen.findByTestId('support-details')
    expect(details.previousElementSibling).toHaveTextContent(primary)
    expect(details.previousElementSibling).not.toHaveTextContent('permission probe diagnostic')
    expect(details).not.toHaveAttribute('open')
    expect(within(details).getByTestId('support-details-text')).toHaveTextContent('permission probe diagnostic')
    expect(attempts).toBe(1)
    expect(screen.getByTestId('onboarding-next')).toBeDisabled()
    expect(consentWrites()).toEqual([])
    fireEvent.click(screen.getByRole('button', { name: i18n.t('settings.permissionRefreshAction') }))
    await waitFor(() => expect(screen.getByTestId('onboarding-next')).toBeEnabled())
    expect(attempts).toBe(2)
    expect(screen.queryByTestId('support-details')).toBeNull()
  })

  it('does not invoke permission IPC in a standalone preview', async () => {
    installBackend()
    startAt(1)
    renderOnboarding()
    await waitFor(() => expect(screen.getByTestId('onboarding-next')).toBeEnabled())
    expect(invokedCommands()).not.toContain('get_desktop_permission_status')
    expect(consentWrites()).toEqual([])
  })

  it.each([
    [
      'accessibility',
      'open_desktop_permission_settings',
      'onboarding.step2AccessibilityAction',
      'Could not open macOS System Settings.',
    ],
    [
      'screen_capture',
      'request_desktop_screen_capture_permission',
      'onboarding.step2ScreenCaptureAction',
      'Could not open macOS System Settings.',
    ],
    [
      'notifications',
      'request_desktop_notification_permission',
      'settings.permissionMacNotificationsRequestAction',
      'Could not request notification permission.',
    ],
  ])('keeps the %s permission action retryable after a null rejection', async (_kind, command, key, primary) => {
    platform.tauri = true
    installBackend()
    const bridge = mockInvoke.getMockImplementation()!
    const entry = { state: 'needs_attention', status_reason: null }
    const snapshot = {
      platform: 'macos',
      accessibility: entry,
      screen_capture: entry,
      microphone: entry,
      input_monitoring: entry,
      notifications: { ...entry, status_reason: 'macos_notifications_not_determined' },
    }
    let attempts = 0
    mockInvoke.mockImplementation((name, args) => {
      if (name === 'get_desktop_permission_status') return Promise.resolve(snapshot)
      if (name === command) {
        if (++attempts === 1) return Promise.reject(null)
        return Promise.resolve({ ...snapshot, screen_capture: { state: 'granted', status_reason: null } })
      }
      return bridge(name, args)
    })
    startAt(1)
    renderOnboarding()
    const button = await screen.findByRole('button', { name: i18n.t(key) })
    await waitFor(() => expect(button).toBeEnabled())
    fireEvent.click(button)
    expect((await screen.findByText(primary)).closest('[role=alert]')).not.toBeNull()
    expect(screen.queryByTestId('support-details')).toBeNull()
    expect(attempts).toBe(1)
    await waitFor(() => expect(button).toBeEnabled())
    fireEvent.click(button)
    await waitFor(() => expect(attempts).toBe(2))
    expect(consentWrites()).toEqual([])
  })

  it.each([
    ['en', 'Something went wrong. Try again.'],
    ['ko', '문제가 발생했습니다. 다시 시도하세요.'],
    ['ja', '問題が発生しました。もう一度お試しください。'],
    ['zh-CN', '出现了问题。请重试。'],
    ['es', 'Algo salió mal. Inténtelo de nuevo.'],
  ])('does not advance or grant silently after a consent failure in %s', async (language, generic) => {
    await i18n.changeLanguage(language)
    const backend = installBackend({ consentWriteError: { code: 'future.consent', message: 'grant diagnostic' } })
    startAt(2)
    renderOnboarding()
    expect(consentWrites()).toEqual([])
    fireEvent.click(screen.getByTestId('onboarding-consent-grant'))
    const details = await screen.findByTestId('support-details')
    expect(details.previousElementSibling).toHaveTextContent(generic)
    expect(details).not.toHaveAttribute('open')
    expect(card()).toHaveAttribute('data-step', '2')
    expect(consentWrites()).toEqual([MONITORING_GRANT])
    backend.consentWriteError = null
    fireEvent.click(screen.getByTestId('onboarding-consent-grant'))
    await expectStep(3)
    expect(consentWrites()).toEqual([MONITORING_GRANT, MONITORING_GRANT])
  })

  it('retains the AI consent DTO detail and real record after an unknown failure', async () => {
    installBackend({
      consent: valid(MONITORING_GRANT),
      consentWriteError: { code: 'future.ai', message: 'AI consent diagnostic' },
    })
    startAt(3)
    renderOnboarding()
    await waitForConsentControls()
    fireEvent.click(screen.getByTestId('onboarding-ai-consent-full-text'))
    const detail = await screen.findByTestId('support-details')
    expect(detail.previousElementSibling).toHaveTextContent('Something went wrong. Try again.')
    expect(within(detail).getByTestId('support-details-text')).toHaveTextContent('future.ai: AI consent diagnostic')
    expect(screen.getByTestId('onboarding-ai-consent-full-text')).not.toBeChecked()
    expect(consentWrites()).toEqual([{ ...MONITORING_GRANT, full_text_extraction: true }])
  })

  it('keeps provider save failures on the AI step without claiming successful application', async () => {
    installBackend({ consent: AI_CONSENT, readiness: NO_PROVIDER })
    mockUpdateSettings.mockRejectedValue(new Error('provider write diagnostic'))
    startAt(3)
    renderOnboarding()
    await expectAiPhase('provider')
    saveAnthropic('test-key')
    const error = await screen.findByTestId('onboarding-ai-save-error')
    const detail = within(error).getByTestId('support-details')
    expect(detail.previousElementSibling).toHaveTextContent('Something went wrong. Try again.')
    expect(detail).not.toHaveAttribute('open')
    expect(mockUpdateSettings).toHaveBeenCalledTimes(1)
    expect(card()).toHaveAttribute('data-step', '3')
    expect(consentWrites()).toEqual([])
  })
  it.each([
    ['en', 'Something went wrong. Try again.'],
    ['ko', '문제가 발생했습니다. 다시 시도하세요.'],
    ['ja', '問題が発生しました。もう一度お試しください。'],
    ['zh-CN', '出现了问题。请重试。'],
    ['es', 'Algo salió mal. Inténtelo de nuevo.'],
  ])('keeps StepReady grant failure recoverable without dropping AI opt-ins in %s', async (language, generic) => {
    await i18n.changeLanguage(language)
    const backend = installBackend({
      consent: valid(permissions({ full_text_extraction: true })),
      consentWriteError: { code: 'future.ready', message: 'ready grant diagnostic' },
    })
    startAt(4)
    const { onComplete } = renderOnboarding()
    const offer = await screen.findByTestId('onboarding-ready-collection-off')
    const grant = within(offer).getByRole('button')
    expect(consentWrites()).toEqual([])
    fireEvent.click(grant)
    const details = await within(offer).findByTestId('support-details')
    expect(details.previousElementSibling).toHaveTextContent(generic)
    expect(details.previousElementSibling).not.toHaveTextContent('ready grant diagnostic')
    expect(details).not.toHaveAttribute('open')
    expect(within(details).getByTestId('support-details-text')).toHaveTextContent(
      'future.ready: ready grant diagnostic',
    )
    const expected = { ...MONITORING_GRANT, full_text_extraction: true }
    expect(consentWrites()).toEqual([expected])
    expect(backend.consent?.permissions.screen_capture).toBe(false)
    expect(onComplete).not.toHaveBeenCalled()
    await waitFor(() => expect(grant).toBeEnabled())
    backend.consentWriteError = null
    fireEvent.click(grant)
    await waitFor(() => expect(screen.queryByTestId('onboarding-ready-collection-off')).toBeNull())
    expect(consentWrites()).toEqual([expected, expected])
    expect(backend.consent?.permissions.full_text_extraction).toBe(true)
    expect(screen.queryByTestId('support-details')).toBeNull()
    expect(onComplete).not.toHaveBeenCalled()
  })
})
