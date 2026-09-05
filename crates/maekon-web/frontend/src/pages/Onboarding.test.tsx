import { fireEvent, screen, waitFor } from '@testing-library/react'
import '@testing-library/jest-dom/vitest'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import type { ConsentPermissions } from '../api/contracts'
import * as aiReadinessModule from '../hooks/useAiReadinessSnapshot'
import en from '../i18n/locales/en.json'
import Onboarding from './Onboarding'

const mockInvoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

vi.mock('../utils/platform', () => ({
  IS_LINUX: false,
  IS_MAC: true,
  IS_TAURI: false,
  IS_WINDOWS: false,
  MOD_KEY: '⌘',
  isTauriRuntime: vi.fn(() => false),
}))

// #5707: stub the HTTP api/client calls used in StepCoaching (fetchSettings,
// updateSettings). getConsent/setConsent route through mockInvoke (IPC).
const mockFetchSettings = vi.fn()
const mockUpdateSettings = vi.fn()
vi.mock('../api/client', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../api/client')>()
  return {
    ...mod,
    fetchSettings: (...args: unknown[]) => mockFetchSettings(...args),
    updateSettings: (...args: unknown[]) => mockUpdateSettings(...args),
  }
})

describe('Onboarding', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockFetchSettings.mockReset()
    mockUpdateSettings.mockReset()
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    })
  })

  afterEach(() => vi.restoreAllMocks())

  it('persists completion when the user skips setup', async () => {
    mockInvoke.mockResolvedValue(undefined)
    const onComplete = vi.fn()

    renderWithProviders(<Onboarding onComplete={onComplete} />)

    fireEvent.click(screen.getByTestId('onboarding-skip'))

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('complete_onboarding')
      expect(onComplete).toHaveBeenCalled()
    })
  })

  it('separates required macOS permissions from recommended notifications', () => {
    renderWithProviders(<Onboarding onComplete={vi.fn()} />)

    fireEvent.click(screen.getByRole('button', { name: 'Next' }))

    expect(screen.getByText(/Accessibility and Screen Recording are required/i)).toBeInTheDocument()
    expect(screen.getByText(/Notifications are recommended/i)).toBeInTheDocument()
    expect(
      screen.queryByText(/requires Accessibility, Screen Recording, and notification access/i),
    ).not.toBeInTheDocument()
  })

  // Intro(0) → Permissions(1) → Consent(2). Because IS_TAURI=false, the Permissions
  // step auto-readies, so Next is never blocked.
  function gotoConsentStep() {
    fireEvent.click(screen.getByRole('button', { name: 'Next' }))
    fireEvent.click(screen.getByRole('button', { name: 'Next' }))
  }

  // #9631: the first-run bundle includes ocr_processing — without it, frames
  // carry no text and /search reads as broken to a new user.
  it('grant CTA → calls set_consent with the 7 monitoring-bundle fields true and the rest false', async () => {
    const invalidateSpy = vi.spyOn(aiReadinessModule, 'invalidateAiReadinessSnapshotCache')
    // The consent IPC returns a grant snapshot (other invokes return undefined).
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'set_consent') {
        const perms = (args as { permissions: ConsentPermissions }).permissions
        return Promise.resolve({ status: 'Valid', permissions: perms })
      }
      return Promise.resolve(undefined)
    })

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoConsentStep()

    // Before granting, the enumeration of collected items must be shown (informational).
    expect(screen.getByText(en.privacy.consent.monitoring.collected.screenFrames)).toBeInTheDocument()
    expect(screen.getByText(en.privacy.consent.monitoring.collected.guiInteractions)).toBeInTheDocument()

    fireEvent.click(screen.getByTestId('onboarding-consent-grant'))

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('set_consent', {
        permissions: {
          screen_capture: true,
          window_title_collection: true,
          app_usage_analytics: true,
          process_monitoring: true,
          input_activity: true,
          telemetry: true,
          ocr_processing: true,
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
        },
      })
      expect(invalidateSpy).toHaveBeenCalledTimes(1)
    })
  })

  it('shows a confirmed state after a successful grant', async () => {
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'set_consent') {
        const perms = (args as { permissions: ConsentPermissions }).permissions
        return Promise.resolve({ status: 'Valid', permissions: perms })
      }
      return Promise.resolve(undefined)
    })

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoConsentStep()

    fireEvent.click(screen.getByTestId('onboarding-consent-grant'))

    await waitFor(() => {
      expect(screen.getByTestId('onboarding-consent-granted')).toBeInTheDocument()
    })
  })

  it('does not call set_consent until the user explicitly clicks grant (not auto-granted)', () => {
    mockInvoke.mockResolvedValue(undefined)

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoConsentStep()

    expect(mockInvoke).not.toHaveBeenCalledWith('set_consent', expect.anything())
  })

  // #9811: StepReady must not claim readiness the app cannot deliver.
  //
  // The consent step deliberately does not gate `Next` — consent has to stay
  // freely refusable — so a user can reach the last step without granting. The
  // app then collects nothing and the timeline stays empty forever, which is
  // exactly what a new install looked like before this fix.

  function gotoReadyStep() {
    // Intro(0)→Permissions(1)→Consent(2)→Features(3)→Audio(4)→Coaching(5)→Ready(6).
    for (let i = 0; i < 6; i += 1) {
      fireEvent.click(screen.getByRole('button', { name: 'Next' }))
    }
  }

  it('says collection is off — not "all set" — when the user skipped consent', async () => {
    // `get_consent` is the only IPC StepReady issues; report a never-granted
    // snapshot, which is what skipping the grant button actually leaves behind.
    mockInvoke.mockImplementation((cmd: string) =>
      cmd === 'get_consent'
        ? Promise.resolve({ status: 'NotGranted', permissions: { screen_capture: false } })
        : Promise.resolve(undefined),
    )

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoReadyStep()

    await waitFor(() => {
      expect(screen.getByTestId('onboarding-ready-collection-off')).toBeInTheDocument()
    })
    expect(screen.queryByText(en.onboarding.step4Desc)).not.toBeInTheDocument()
  })

  it('grants from the last step without forcing it earlier', async () => {
    let granted = false
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_consent') {
        return Promise.resolve(
          granted
            ? { status: 'Valid', permissions: { screen_capture: true } }
            : { status: 'NotGranted', permissions: { screen_capture: false } },
        )
      }
      if (cmd === 'set_consent') {
        granted = true
        return Promise.resolve({ status: 'Valid', permissions: { screen_capture: true } })
      }
      return Promise.resolve(undefined)
    })

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoReadyStep()

    await waitFor(() => {
      expect(screen.getByTestId('onboarding-ready-collection-off')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('onboarding-ready-collection-off').querySelector('button')!)

    // The warning clears only because consent actually landed, not because the
    // button was pressed — the re-read is what decides.
    await waitFor(() => {
      expect(screen.queryByTestId('onboarding-ready-collection-off')).not.toBeInTheDocument()
    })
    expect(mockInvoke).toHaveBeenCalledWith('set_consent', expect.anything())
  })

  it('stays quiet when consent is already granted', async () => {
    mockInvoke.mockImplementation((cmd: string) =>
      cmd === 'get_consent'
        ? Promise.resolve({ status: 'Valid', permissions: { screen_capture: true } })
        : Promise.resolve(undefined),
    )

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoReadyStep()

    await waitFor(() => {
      expect(screen.getByText(en.onboarding.step4Desc)).toBeInTheDocument()
    })
    expect(screen.queryByTestId('onboarding-ready-collection-off')).not.toBeInTheDocument()
  })

  // #5707: StepCoaching tests — index 5 of 7 (after the default-off Audio step).
  // Intro(0)→Permissions(1)→Consent(2)→Features(3)→Audio(4)→Coaching(5).
  // IS_TAURI=false means PermissionsStep auto-readies; Next is never blocked.

  function gotoCoachingStep() {
    // From step 0, click Next five times to reach step 5.
    fireEvent.click(screen.getByRole('button', { name: 'Next' })) // 0→1
    fireEvent.click(screen.getByRole('button', { name: 'Next' })) // 1→2
    fireEvent.click(screen.getByRole('button', { name: 'Next' })) // 2→3
    fireEvent.click(screen.getByRole('button', { name: 'Next' })) // 3→4
    fireEvent.click(screen.getByRole('button', { name: 'Next' })) // 4→5
  }

  it('renders StepCoaching at index 5 with the Enable button', async () => {
    mockInvoke.mockResolvedValue(undefined)

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoCoachingStep()

    // The coaching enable button must be present before any click.
    await waitFor(() => {
      expect(screen.getByTestId('onboarding-coaching-enable')).toBeInTheDocument()
    })
    // The "already enabled" confirmation must NOT be present yet.
    expect(screen.queryByTestId('onboarding-coaching-enabled')).not.toBeInTheDocument()
  })

  it('clicking Enable coaching calls get_consent → set_consent → fetchSettings → updateSettings', async () => {
    const invalidateSpy = vi.spyOn(aiReadinessModule, 'invalidateAiReadinessSnapshotCache')
    // IPC stubs: get_consent returns a minimal snapshot; set_consent echoes it.
    const basePermissions: ConsentPermissions = {
      screen_capture: true,
      window_title_collection: true,
      app_usage_analytics: true,
      process_monitoring: true,
      input_activity: true,
      telemetry: true,
      ocr_processing: false,
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
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'get_consent') {
        return Promise.resolve({ status: 'Valid', permissions: basePermissions })
      }
      if (cmd === 'set_consent') {
        const perms = (args as { permissions: ConsentPermissions }).permissions
        return Promise.resolve({ status: 'Valid', permissions: perms })
      }
      return Promise.resolve(undefined)
    })
    // HTTP stubs for fetchSettings / updateSettings.
    const settingsBase = { coaching: { enabled: false } }
    mockFetchSettings.mockResolvedValue(settingsBase)
    mockUpdateSettings.mockResolvedValue({ ...settingsBase, coaching: { enabled: true } })

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoCoachingStep()

    const enableBtn = await screen.findByTestId('onboarding-coaching-enable')
    fireEvent.click(enableBtn)

    await waitFor(() => {
      // Tier-4 consent must be merged: activity_pattern_learning → true.
      expect(mockInvoke).toHaveBeenCalledWith('set_consent', {
        permissions: { ...basePermissions, activity_pattern_learning: true },
      })
      // HTTP settings write must have been called with coaching.enabled = true.
      expect(mockUpdateSettings).toHaveBeenCalledWith(
        expect.objectContaining({ coaching: expect.objectContaining({ enabled: true }) }),
      )
      expect(invalidateSpy).toHaveBeenCalledTimes(1)
    })
  })

  it('shows the confirmed state after Enable coaching succeeds', async () => {
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'get_consent') {
        return Promise.resolve({ status: 'Valid', permissions: {} })
      }
      if (cmd === 'set_consent') {
        const perms = (args as { permissions: ConsentPermissions }).permissions
        return Promise.resolve({ status: 'Valid', permissions: perms })
      }
      return Promise.resolve(undefined)
    })
    mockFetchSettings.mockResolvedValue({ coaching: { enabled: false } })
    mockUpdateSettings.mockResolvedValue({ coaching: { enabled: true } })

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoCoachingStep()

    const enableBtn = await screen.findByTestId('onboarding-coaching-enable')
    fireEvent.click(enableBtn)

    await waitFor(() => {
      expect(screen.getByTestId('onboarding-coaching-enabled')).toBeInTheDocument()
    })
  })

  it('default-off guard: coaching Enable button is NOT auto-clicked — coaching stays off if user skips', () => {
    // No mocks fire — if the button were auto-clicked, get_consent IPC would fire.
    mockInvoke.mockResolvedValue(undefined)

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoCoachingStep()

    // get_consent must not have been called (no auto-grant).
    expect(mockInvoke).not.toHaveBeenCalledWith('get_consent')
  })

  // #8059 G2b: StepFeatures (index 3) discoverability + AI-features opt-in.
  // Intro(0)→Permissions(1)→Consent(2)→Features(3). IS_TAURI=false auto-readies
  // the Permissions step, so Next is never blocked.
  function gotoFeaturesStep() {
    fireEvent.click(screen.getByRole('button', { name: 'Next' })) // 0→1
    fireEvent.click(screen.getByRole('button', { name: 'Next' })) // 1→2
    fireEvent.click(screen.getByRole('button', { name: 'Next' })) // 2→3
  }

  it('shows an explicit default-off audio defer step without writing settings', () => {
    mockInvoke.mockResolvedValue(undefined)

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoFeaturesStep()
    fireEvent.click(screen.getByRole('button', { name: 'Next' })) // 3→4

    expect(screen.getByTestId('onboarding-audio-deferred')).toBeInTheDocument()
    expect(screen.getByText(en.onboarding.audio.statusTitle)).toBeInTheDocument()
    expect(screen.getByText(en.onboarding.audio.egressNote)).toBeInTheDocument()
    expect(mockUpdateSettings).not.toHaveBeenCalled()
    expect(mockInvoke).not.toHaveBeenCalledWith('set_consent', expect.anything())
  })

  it('StepFeatures lists a Search feature and shows the AI-features opt-in button (not pre-enabled)', () => {
    mockInvoke.mockResolvedValue(undefined)

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoFeaturesStep()

    // The new fourth feature (Search) must be listed.
    expect(screen.getByText(en.onboarding.step3Search)).toBeInTheDocument()
    // Opt-in button present; confirmed state not yet shown.
    expect(screen.getByTestId('onboarding-aifeatures-enable')).toBeInTheDocument()
    expect(screen.queryByTestId('onboarding-aifeatures-prepared')).not.toBeInTheDocument()
  })

  it('prepares all four summary flags without claiming runtime readiness', async () => {
    const invalidateSpy = vi.spyOn(aiReadinessModule, 'invalidateAiReadinessSnapshotCache')
    mockInvoke.mockResolvedValue(undefined)
    mockFetchSettings.mockResolvedValue({
      analysis: {
        enabled: false,
        tiered_memory_enabled: false,
        embedding_enabled: false,
        llm_summary_enabled: false,
        interval_secs: 60,
      },
    })
    mockUpdateSettings.mockResolvedValue({
      analysis: {
        enabled: true,
        tiered_memory_enabled: true,
        embedding_enabled: true,
        llm_summary_enabled: true,
        interval_secs: 60,
      },
    })

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoFeaturesStep()

    fireEvent.click(screen.getByTestId('onboarding-aifeatures-enable'))

    await waitFor(() => {
      expect(mockUpdateSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          analysis: expect.objectContaining({
            enabled: true,
            tiered_memory_enabled: true,
            embedding_enabled: true,
            llm_summary_enabled: true,
          }),
        }),
      )
      expect(screen.getByTestId('onboarding-aifeatures-prepared')).toBeInTheDocument()
      expect(screen.getByText(en.onboarding.aiFeatures.preparedNote)).toBeInTheDocument()
      expect(invalidateSpy).toHaveBeenCalledTimes(1)
    })
  })

  it('default-off guard: AI-features opt-in is NOT auto-enabled — updateSettings is not called if the user skips', () => {
    mockInvoke.mockResolvedValue(undefined)

    renderWithProviders(<Onboarding onComplete={vi.fn()} />)
    gotoFeaturesStep()

    // No settings write may fire without an explicit click.
    expect(mockUpdateSettings).not.toHaveBeenCalled()
  })
})
