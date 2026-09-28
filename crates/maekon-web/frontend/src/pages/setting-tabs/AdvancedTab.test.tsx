/**
 * AdvancedTab tiered-memory disclosure (#9687).
 *
 * Turning the tiered-memory switch on is NOT sufficient: the pipeline is also
 * gated on the activity_pattern_learning consent (off by default, granted on
 * Privacy → Data Controls under the "Pattern Learning & Coaching" toggle) and is
 * wired at startup from a config snapshot, so it only takes effect at the next
 * launch. Both were verified against the running app; without these notices a
 * user flips the switch and sees no change anywhere.
 *
 * The notices must also distinguish "consent is missing" from "consent could not
 * be read" — the consent read is Tauri IPC, and this frontend has a real
 * standalone mode where it always fails.
 *
 * Negative assertions here go through ROLES, not copy: `Alert` maps warning ->
 * role="alert" and info -> role="status", and this tab declares no other role.
 * Copy-keyed negatives pass vacuously as soon as the wording changes, and this
 * wording has already been revised twice — the positive assertions are where
 * pinning the exact words is the point.
 */

import { fireEvent, screen, waitFor } from '@testing-library/react'
import '@testing-library/jest-dom/vitest'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import type { AiReadinessReasonCode, ConsentSnapshot, FeatureCapabilitySnapshot } from '../../api/contracts'
import AdvancedTab from './AdvancedTab'
import { makeDefaultFormData } from './stories-utils'

const mockUseSettingsFormContext = vi.hoisted(() => vi.fn())
const mockGetConsent = vi.hoisted(() => vi.fn())
const mockUseAiReadinessSnapshot = vi.hoisted(() => vi.fn())

vi.mock('../settings/SettingsFormContext', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../settings/SettingsFormContext')>()
  return {
    ...mod,
    useSettingsFormContext: mockUseSettingsFormContext,
    useLoadedFormData: () => mockUseSettingsFormContext().form.formData,
  }
})

vi.mock('../../api/client', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../../api/client')>()
  return { ...mod, getConsent: () => mockGetConsent() }
})

vi.mock('../../hooks/useAiReadinessSnapshot', () => ({
  useAiReadinessSnapshot: () => mockUseAiReadinessSnapshot(),
}))

function ocrReadinessSnapshot(
  reasonCode: AiReadinessReasonCode,
  action: 'enable_feature' | 'open_ai_settings',
): FeatureCapabilitySnapshot {
  return {
    features: [],
    ai_readiness: {
      contract_version: 1,
      capabilities: [
        {
          capability_id: 'ocr.suggestion_analysis',
          status: 'blocked',
          reason_code: reasonCode,
          action,
          action_copy_key:
            action === 'enable_feature' ? 'aiReadiness.action.enableFeature' : 'aiReadiness.action.openAiSettings',
          dimensions: {
            compiled_capability: true,
            selected_access_mode: 'provider_api_key',
            access_mode_compatible: reasonCode !== 'access_mode_mismatch',
            endpoint_or_profile_configured: false,
            provider_detection: 'detected',
            provider_auth: 'ready',
            provider_invocation: 'ready',
            model_availability: 'not_required',
            runtime_flag_enabled: reasonCode !== 'runtime_flag_disabled',
            consent: [
              { field: 'ocr_processing', granted: true },
              { field: 'activity_pattern_learning', granted: true },
            ],
            apply_requirement: 'hot_rewire',
            apply_pending: false,
            privacy_gate: 'enforced_at_invocation',
            egress_gate: 'enforced_at_invocation',
            budget_gate: 'enforced_at_invocation',
            audit_gate: 'enforced_at_invocation',
          },
        },
      ],
    },
  }
}

function consentSnapshot(patternLearning: boolean): ConsentSnapshot {
  return {
    status: 'Valid',
    permissions: {
      screen_capture: true,
      ocr_processing: true,
      telemetry: false,
      process_monitoring: true,
      input_activity: true,
      window_title_collection: true,
      app_usage_analytics: true,
      clipboard_monitoring: false,
      file_access_monitoring: false,
      activity_pattern_learning: patternLearning,
      cross_device_sync: false,
      full_text_extraction: false,
      memory_graph_enrichment: false,
      microphone: false,
      unredacted_external_ocr: false,
      memory_graph_retrieval_ranking: false,
      memory_vault_mirror: false,
    },
  } as ConsentSnapshot
}

function renderWithTieredMemory(enabled: boolean) {
  const formData = makeDefaultFormData()
  formData.analysis.tiered_memory_enabled = enabled
  const setFormData = vi.fn()
  mockUseSettingsFormContext.mockReturnValue({
    form: { formData, setFormData, handleRootChange: vi.fn() },
  })
  renderWithProviders(<AdvancedTab />)
  return { formData, setFormData }
}

describe('AdvancedTab tiered-memory disclosure (#9687)', () => {
  beforeEach(() => {
    mockUseSettingsFormContext.mockReset()
    mockGetConsent.mockReset()
    mockUseAiReadinessSnapshot.mockReset()
    mockUseAiReadinessSnapshot.mockReturnValue(undefined)
  })

  it('labels server SSE and OCR-derived analysis as separate producers', async () => {
    mockGetConsent.mockResolvedValue(consentSnapshot(true))
    renderWithTieredMemory(false)

    expect(await screen.findByText('Receive server suggestions')).toBeInTheDocument()
    expect(screen.getByText('Generate local activity suggestions')).toBeInTheDocument()
    expect(screen.getByText(/signed-in ONESHIM server stream/i)).toBeInTheDocument()
    expect(screen.getByText(/consented local activity and OCR context/i)).toBeInTheDocument()
  })

  it('prepares all four summary prerequisites without granting consent', async () => {
    mockGetConsent.mockResolvedValue(consentSnapshot(false))
    const { formData, setFormData } = renderWithTieredMemory(false)
    formData.analysis.enabled = false
    formData.analysis.embedding_enabled = false
    formData.analysis.llm_summary_enabled = false

    fireEvent.click(screen.getByRole('checkbox', { name: /^Enable AI features/ }))

    const updater = setFormData.mock.calls.at(-1)?.[0]
    expect(typeof updater).toBe('function')
    const updated = updater(formData)
    expect(updated.analysis).toMatchObject({
      enabled: true,
      tiered_memory_enabled: true,
      embedding_enabled: true,
      llm_summary_enabled: true,
    })
    expect(updated).not.toHaveProperty('consent')
  })

  it('shows the backend-owned analysis-disabled blocker and setup action', async () => {
    mockGetConsent.mockResolvedValue(consentSnapshot(true))
    mockUseAiReadinessSnapshot.mockReturnValue(ocrReadinessSnapshot('runtime_flag_disabled', 'enable_feature'))
    renderWithTieredMemory(false)

    const blocker = document.querySelector('li[data-reason-code="runtime_flag_disabled"]')
    expect(blocker).not.toBeNull()
    expect(screen.getByRole('link', { name: /enable the required feature/i })).toHaveAttribute(
      'href',
      '/settings/ai-automation',
    )
  })

  it('shows provider access-mode mismatch separately from disabled analysis', async () => {
    mockGetConsent.mockResolvedValue(consentSnapshot(true))
    mockUseAiReadinessSnapshot.mockReturnValue(ocrReadinessSnapshot('access_mode_mismatch', 'open_ai_settings'))
    renderWithTieredMemory(false)

    expect(document.querySelector('li[data-reason-code="access_mode_mismatch"]')).not.toBeNull()
    expect(document.querySelector('li[data-reason-code="runtime_flag_disabled"]')).toBeNull()
    expect(screen.getByRole('link', { name: /open ai settings/i })).toHaveAttribute('href', '/settings/ai-automation')
  })

  it('shows neither notice when the consent cannot be read at all', async () => {
    // Standalone mode / no Tauri / transient IPC error. Treating this as
    // "consent denied" would tell a user who DID grant it that they did not.
    mockGetConsent.mockRejectedValue(new Error('Tauri unavailable'))
    renderWithTieredMemory(true)

    await waitFor(() => {
      expect(screen.getByText(/Tiered memory pipeline/i)).toBeInTheDocument()
    })
    await waitFor(() => {
      expect(mockGetConsent).toHaveBeenCalled()
    })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.queryByRole('status')).not.toBeInTheDocument()
  })

  it('shows neither notice while the consent read is still in flight', async () => {
    // A promise that never settles — the cold-mount frame. Rendering the
    // warning here would flash a false "consent missing" for every user.
    mockGetConsent.mockReturnValue(new Promise(() => {}))
    renderWithTieredMemory(true)

    expect(await screen.findByText(/Tiered memory pipeline/i)).toBeInTheDocument()
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.queryByRole('status')).not.toBeInTheDocument()
  })

  it('warns that the pipeline also needs the pattern-learning consent', async () => {
    mockGetConsent.mockResolvedValue(consentSnapshot(false))
    renderWithTieredMemory(true)

    await waitFor(() => {
      expect(screen.getByText(/Privacy → Data Controls/)).toBeInTheDocument()
    })
    expect(screen.queryByRole('status')).not.toBeInTheDocument()
  })

  it('asks for a restart once the consent is in place', async () => {
    mockGetConsent.mockResolvedValue(consentSnapshot(true))
    renderWithTieredMemory(true)

    await waitFor(() => {
      expect(screen.getByText(/Takes effect at the next launch/i)).toBeInTheDocument()
    })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('shows neither notice while the switch is off', async () => {
    mockGetConsent.mockResolvedValue(consentSnapshot(false))
    renderWithTieredMemory(false)

    await waitFor(() => {
      expect(screen.getByText(/Tiered memory pipeline/i)).toBeInTheDocument()
    })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.queryByRole('status')).not.toBeInTheDocument()
  })
})
