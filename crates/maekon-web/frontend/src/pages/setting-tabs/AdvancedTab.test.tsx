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

import { screen, waitFor } from '@testing-library/react'
import '@testing-library/jest-dom/vitest'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import type { ConsentSnapshot } from '../../api/contracts'
import AdvancedTab from './AdvancedTab'
import { makeDefaultFormData } from './stories-utils'

const mockUseSettingsFormContext = vi.hoisted(() => vi.fn())
const mockGetConsent = vi.hoisted(() => vi.fn())

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
  mockUseSettingsFormContext.mockReturnValue({
    form: { formData, setFormData: vi.fn(), handleRootChange: vi.fn() },
  })
  renderWithProviders(<AdvancedTab />)
}

describe('AdvancedTab tiered-memory disclosure (#9687)', () => {
  beforeEach(() => {
    mockUseSettingsFormContext.mockReset()
    mockGetConsent.mockReset()
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
