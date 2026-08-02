/**
 * ConsentToggleSection behavior test (#4629 A.2 task 2).
 *
 * Verifies *behavior* rather than copy (wording) — i.e. which IPC invoke is called with which
 * arguments. (The actual i18n key strings are added in Task 3, and since i18next returns the key
 * itself when the key is missing, the test is green even if t() returns the key verbatim.)
 *
 * Core contract (Task 1 verification criteria):
 *   - get_consent returns the RAW granted permissions even when status is Expired/UpdateRequired.
 *     Therefore the "monitoring ON" decision is based solely on status === 'Valid' AND the field.
 *   - set_consent replaces the record wholesale, so every handler spreads the entire current set of
 *     permissions and then flips only the target field before sending (to avoid losing other opt-ins).
 */

import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { ConsentPermissions, ConsentSnapshot, ConsentStatus } from '../../../api/contracts'
// Test i18n resolves to en (fallbackLng='en'). After Task 3 keys resolve to actual copy, so
// reference the resolved copy from en.json directly instead of the key string (the test stays
// consistent across wording changes as long as the key path is preserved).
import * as toastModule from '../../../hooks/useToast'
import en from '../../../i18n/locales/en.json'
import ConsentToggleSection from '../ConsentToggleSection'

// Baseline permission set with all 17 permissions false.
const ALL_FALSE: ConsentPermissions = {
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

function snapshot(status: ConsentStatus, overrides: Partial<ConsentPermissions> = {}): ConsentSnapshot {
  return { status, permissions: { ...ALL_FALSE, ...overrides } }
}

/**
 * Mocks the IPC so that get_consent always returns the given snapshot, and set_consent /
 * withdraw_consent record the passed arguments via a spy and then return a suitable snapshot.
 */
function mockConsentIpc(initial: ConsentSnapshot) {
  const setSpy = vi.fn()
  const withdrawSpy = vi.fn()
  mockIPC((cmd, args) => {
    if (cmd === 'get_consent') {
      return initial
    }
    if (cmd === 'set_consent') {
      setSpy(args)
      const permissions = (args as { permissions: ConsentPermissions }).permissions
      return { status: 'Valid', permissions } satisfies ConsentSnapshot
    }
    if (cmd === 'withdraw_consent') {
      withdrawSpy(args)
      return snapshot('NotGranted')
    }
    return undefined
  })
  return { setSpy, withdrawSpy }
}

describe('ConsentToggleSection', () => {
  afterEach(() => clearMocks())

  it('mounts → calls get_consent and reflects a Valid+screen_capture snapshot as monitoring ON', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const toggle = await screen.findByTestId('consent-monitoring-toggle')
    await waitFor(() => expect(toggle).toBeChecked())
  })

  it('toggling monitoring ON from a NotGranted snapshot → set_consent with the 6 master fields true (others preserved)', async () => {
    // Assume ocr_processing is already separately enabled → it must be preserved by the spread.
    const { setSpy } = mockConsentIpc(snapshot('NotGranted', { ocr_processing: true }))
    const onConsentChanged = vi.fn()

    renderWithProviders(<ConsentToggleSection onConsentChanged={onConsentChanged} />)

    const toggle = await screen.findByTestId('consent-monitoring-toggle')
    await waitFor(() => expect(toggle).not.toBeChecked())
    fireEvent.click(toggle)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    // The 6 master fields ON
    expect(sent.screen_capture).toBe(true)
    expect(sent.window_title_collection).toBe(true)
    expect(sent.app_usage_analytics).toBe(true)
    expect(sent.process_monitoring).toBe(true)
    expect(sent.input_activity).toBe(true)
    expect(sent.telemetry).toBe(true)
    // Separate opt-in preserved (spread)
    expect(sent.ocr_processing).toBe(true)
    // High-sensitivity opt-ins are left untouched
    expect(sent.clipboard_monitoring).toBe(false)
    expect(sent.file_access_monitoring).toBe(false)
    expect(onConsentChanged).toHaveBeenCalledTimes(1)
  })

  it('toggling the microphone opt-in → set_consent flips microphone, all other fields preserved (full-spread)', async () => {
    // Monitoring is already on (Valid) and clipboard is also separately on; turn on only the microphone in addition.
    const { setSpy } = mockConsentIpc(
      snapshot('Valid', {
        screen_capture: true,
        window_title_collection: true,
        app_usage_analytics: true,
        process_monitoring: true,
        input_activity: true,
        telemetry: true,
        clipboard_monitoring: true,
        file_access_monitoring: true,
      }),
    )

    renderWithProviders(<ConsentToggleSection />)

    const microphone = await screen.findByTestId('consent-microphone-toggle')
    await waitFor(() => expect(microphone).not.toBeChecked())
    fireEvent.click(microphone)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.microphone).toBe(true)
    // All other fields are preserved by the spread.
    expect(sent.screen_capture).toBe(true)
    expect(sent.window_title_collection).toBe(true)
    expect(sent.telemetry).toBe(true)
    expect(sent.clipboard_monitoring).toBe(true)
    expect(sent.file_access_monitoring).toBe(true)
  })

  it('toggling the unredacted external OCR opt-in → set_consent flips only that consent tier', async () => {
    const { setSpy } = mockConsentIpc(
      snapshot('Valid', {
        screen_capture: true,
        ocr_processing: true,
        clipboard_monitoring: true,
        microphone: true,
      }),
    )

    renderWithProviders(<ConsentToggleSection />)

    const rawOcr = await screen.findByTestId('consent-unredacted-external-ocr-toggle')
    await waitFor(() => expect(rawOcr).not.toBeChecked())
    fireEvent.click(rawOcr)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.unredacted_external_ocr).toBe(true)
    expect(sent.screen_capture).toBe(true)
    expect(sent.ocr_processing).toBe(true)
    expect(sent.clipboard_monitoring).toBe(true)
    expect(sent.microphone).toBe(true)
  })

  it('toggling OCR processing → set_consent flips only ocr_processing and preserves every other consent field', async () => {
    const { setSpy } = mockConsentIpc(
      snapshot('Valid', {
        screen_capture: true,
        clipboard_monitoring: true,
        file_access_monitoring: true,
        microphone: true,
      }),
    )

    renderWithProviders(<ConsentToggleSection />)

    const ocr = await screen.findByTestId('consent-ocr-processing-toggle')
    await waitFor(() => expect(ocr).not.toBeChecked())
    fireEvent.click(ocr)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.ocr_processing).toBe(true)
    expect(sent.screen_capture).toBe(true)
    expect(sent.clipboard_monitoring).toBe(true)
    expect(sent.file_access_monitoring).toBe(true)
    expect(sent.microphone).toBe(true)
    expect(sent.unredacted_external_ocr).toBe(false)
  })

  it('toggling full-text AI consent flips only full_text_extraction and preserves every other field', async () => {
    const { setSpy } = mockConsentIpc(
      snapshot('Valid', {
        screen_capture: true,
        ocr_processing: true,
        clipboard_monitoring: true,
        microphone: true,
      }),
    )

    renderWithProviders(<ConsentToggleSection />)

    const fullText = await screen.findByTestId('consent-full-text-toggle')
    await waitFor(() => expect(fullText).not.toBeChecked())
    fireEvent.click(fullText)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.full_text_extraction).toBe(true)
    expect(sent.screen_capture).toBe(true)
    expect(sent.ocr_processing).toBe(true)
    expect(sent.clipboard_monitoring).toBe(true)
    expect(sent.microphone).toBe(true)
    expect(sent.unredacted_external_ocr).toBe(false)
  })

  // #8686 AC2: the three onboarding-granted opt-ins must be revocable
  // granularly — previously only the nuclear withdraw path removed them.
  it('toggling pattern learning OFF → set_consent flips only activity_pattern_learning', async () => {
    const { setSpy } = mockConsentIpc(
      snapshot('Valid', {
        screen_capture: true,
        activity_pattern_learning: true,
        cross_device_sync: true,
      }),
    )

    renderWithProviders(<ConsentToggleSection />)

    const toggle = await screen.findByTestId('consent-pattern-learning-toggle')
    await waitFor(() => expect(toggle).toBeChecked())
    fireEvent.click(toggle)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.activity_pattern_learning).toBe(false)
    expect(sent.cross_device_sync).toBe(true)
    expect(sent.screen_capture).toBe(true)
  })

  it('toggling cross-device sync OFF → set_consent flips only cross_device_sync', async () => {
    const { setSpy } = mockConsentIpc(
      snapshot('Valid', {
        cross_device_sync: true,
        memory_graph_enrichment: true,
      }),
    )

    renderWithProviders(<ConsentToggleSection />)

    const toggle = await screen.findByTestId('consent-cross-device-sync-toggle')
    await waitFor(() => expect(toggle).toBeChecked())
    fireEvent.click(toggle)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.cross_device_sync).toBe(false)
    expect(sent.memory_graph_enrichment).toBe(true)
  })

  it('toggling memory graph enrichment ON → set_consent flips only memory_graph_enrichment', async () => {
    const { setSpy } = mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const toggle = await screen.findByTestId('consent-memory-graph-toggle')
    await waitFor(() => expect(toggle).not.toBeChecked())
    fireEvent.click(toggle)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.memory_graph_enrichment).toBe(true)
    expect(sent.screen_capture).toBe(true)
    expect(sent.activity_pattern_learning).toBe(false)
  })

  it('associates the external-AI disclosure with the full-text toggle via aria-describedby', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const fullText = await screen.findByTestId('consent-full-text-toggle')
    const describedBy = fullText.getAttribute('aria-describedby')
    expect(describedBy).toBeTruthy()
    const described = document.getElementById(describedBy as string)
    expect(described).not.toBeNull()
    expect(described).toHaveTextContent(en.privacy.consent.fullText.disclosure)
    expect(described?.getAttribute('role')).toBe('alert')
  })

  it('associates the OCR processing boundary disclosure with its toggle via aria-describedby', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const ocr = await screen.findByTestId('consent-ocr-processing-toggle')
    const describedBy = ocr.getAttribute('aria-describedby')
    expect(describedBy).toBeTruthy()
    const described = document.getElementById(describedBy as string)
    expect(described).not.toBeNull()
    expect(described).toHaveTextContent(en.privacy.consent.ocrProcessing.disclosure)
    expect(described?.getAttribute('role')).toBe('status')
  })

  it('associates the unredacted external OCR disclosure with its toggle via aria-describedby', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const rawOcr = await screen.findByTestId('consent-unredacted-external-ocr-toggle')
    const describedBy = rawOcr.getAttribute('aria-describedby')
    expect(describedBy).toBeTruthy()
    const described = document.getElementById(describedBy as string)
    expect(described).not.toBeNull()
    expect(described).toHaveTextContent(en.privacy.consent.unredactedExternalOcr.disclosure)
    expect(described?.getAttribute('role')).toBe('alert')
  })

  it('an Expired snapshot reads the microphone toggle as OFF even though microphone is true (status gating)', async () => {
    mockConsentIpc(snapshot('Expired', { microphone: true }))

    renderWithProviders(<ConsentToggleSection />)

    const microphone = await screen.findByTestId('consent-microphone-toggle')
    // Since status !== 'Valid', the toggle is OFF even if the RAW permission is true (fail-closed display).
    await waitFor(() => expect(microphone).not.toBeChecked())
  })

  it('renders the microphone cloud-egress disclosure as a visible warning (role="alert")', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    // The disclosure renders as a visible warning via Alert variant="warning" → role="alert".
    // (A statically-rendered role="alert" is not reliably announced — WAI-ARIA: auto-announced only
    // when inserted after load — so it is treated as a visible warning + toggle-association (test
    // below) rather than as something "announced".)
    const disclosure = await screen.findByText(en.privacy.consent.microphone.disclosure)
    expect(disclosure).toBeInTheDocument()
    // It renders inside the nearest role="alert" container (Alert maps warning→role=alert).
    expect(disclosure.closest('[role="alert"]')).not.toBeNull()
  })

  // a11y: since a static role="alert" is not reliably announced (WAI-ARIA), *associate* the
  // disclosure with the microphone toggle via aria-describedby — so the screen reader also reads
  // the cloud-egress disclosure when it reads the microphone toggle. (deep review POLISH 2.)
  it('associates the cloud-egress disclosure with the mic toggle via aria-describedby', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const microphone = await screen.findByTestId('consent-microphone-toggle')
    // The microphone checkbox must have an aria-describedby pointing to the disclosure.
    const describedBy = microphone.getAttribute('aria-describedby')
    expect(describedBy).toBeTruthy()
    // The node pointed to by that id must be the (role="alert") element holding the disclosure copy.
    const described = document.getElementById(describedBy as string)
    expect(described).not.toBeNull()
    expect(described).toHaveTextContent(en.privacy.consent.microphone.disclosure)
    expect(described?.getAttribute('role')).toBe('alert')
  })

  it('toggling clipboard opt-in → set_consent flips clipboard_monitoring/file_access_monitoring, other fields preserved', async () => {
    // Monitoring is already on (Valid); turn on only the clipboard in addition.
    const { setSpy } = mockConsentIpc(
      snapshot('Valid', {
        screen_capture: true,
        window_title_collection: true,
        app_usage_analytics: true,
        process_monitoring: true,
        input_activity: true,
        telemetry: true,
      }),
    )

    renderWithProviders(<ConsentToggleSection />)

    const clipboard = await screen.findByTestId('consent-clipboard-toggle')
    await waitFor(() => expect(clipboard).not.toBeChecked())
    fireEvent.click(clipboard)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.clipboard_monitoring).toBe(true)
    expect(sent.file_access_monitoring).toBe(true)
    // The monitoring master fields are not lost (spread).
    expect(sent.screen_capture).toBe(true)
    expect(sent.window_title_collection).toBe(true)
    expect(sent.telemetry).toBe(true)
  })

  it('withdraw action → calls withdraw_consent', async () => {
    const { withdrawSpy } = mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    // Withdrawal goes through a confirm modal: trigger → the modal's confirm button.
    // The confirm button is the danger button that ConfirmModal labels with confirmText (resolved copy).
    const trigger = await screen.findByTestId('consent-withdraw-trigger')
    fireEvent.click(trigger)
    const confirm = await screen.findByRole('button', {
      name: en.privacy.consent.withdraw.confirmButton,
    })
    fireEvent.click(confirm)

    await waitFor(() => expect(withdrawSpy).toHaveBeenCalledTimes(1))
  })

  it('#9505: withdraw failure surfaces the mapped error toast and keeps the modal open for retry', async () => {
    const addToastSpy = vi.spyOn(toastModule, 'addToast')
    // The real failure shape: `withdraw_consent` rejects with the
    // `storage.unavailable` IpcError from erase_all_local_data. Previously this
    // was completely silent — no onError, and the confirm modal covered the
    // card's inline alert.
    mockIPC((cmd) => {
      if (cmd === 'get_consent') return snapshot('Valid', { screen_capture: true })
      if (cmd === 'withdraw_consent') {
        return Promise.reject({ code: 'storage.unavailable', message: 'frame purge unverified' })
      }
      return undefined
    })

    renderWithProviders(<ConsentToggleSection />)

    const trigger = await screen.findByTestId('consent-withdraw-trigger')
    fireEvent.click(trigger)
    const confirm = await screen.findByRole('button', {
      name: en.privacy.consent.withdraw.confirmButton,
    })
    fireEvent.click(confirm)

    await waitFor(() => {
      expect(addToastSpy).toHaveBeenCalledWith('error', en.errors.ipc.storageUnavailable)
    })
    // Not the raw Rust wire literal.
    expect(addToastSpy).not.toHaveBeenCalledWith('error', expect.stringContaining('frame purge unverified'))
    // The modal stays open as the retry affordance (sibling parity with the
    // PrivacyLayout delete-all path) — closing silently would look like success.
    expect(screen.getByRole('button', { name: en.privacy.consent.withdraw.confirmButton })).toBeInTheDocument()

    addToastSpy.mockRestore()
  })

  it('#9524: confirm button is disabled while the withdraw erase is in flight', async () => {
    // A never-resolving withdraw keeps the mutation pending; the confirm
    // button must not accept a second click (double-submit would run two
    // concurrent erase passes, each with its own EraseWindowGuard).
    mockIPC((cmd) => {
      if (cmd === 'get_consent') return snapshot('Valid', { screen_capture: true })
      if (cmd === 'withdraw_consent') return new Promise(() => {})
      return undefined
    })

    renderWithProviders(<ConsentToggleSection />)

    fireEvent.click(await screen.findByTestId('consent-withdraw-trigger'))
    const confirm = await screen.findByRole('button', {
      name: en.privacy.consent.withdraw.confirmButton,
    })
    expect(confirm).toBeEnabled()
    fireEvent.click(confirm)

    await waitFor(() => expect(confirm).toBeDisabled())
  })

  it('an Expired snapshot renders a visible warning', async () => {
    mockConsentIpc(snapshot('Expired', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    // The Expired warning is exposed via Alert variant="warning" → role="alert".
    const warning = await screen.findByText(en.privacy.consent.status.expiredWarning)
    expect(warning).toBeInTheDocument()
  })

  it('an Expired snapshot reads the monitoring toggle as OFF even though screen_capture is true (status gating)', async () => {
    mockConsentIpc(snapshot('Expired', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const toggle = await screen.findByTestId('consent-monitoring-toggle')
    // Since status !== 'Valid', the toggle is OFF even if the RAW permission is true (fail-closed display).
    await waitFor(() => expect(toggle).not.toBeChecked())
  })

  // F6 (a11y): each consent toggle must have an accessible name identical to its visible label — so
  // the screen reader can read the toggle's purpose from the label copy rather than "checkbox, unchecked".
  it('exposes each consent toggle to assistive tech with its visible label as the accessible name', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    // Each toggle must resolve by role (role=checkbox) + accessible name (name=visible label copy).
    const monitoring = await screen.findByRole('checkbox', {
      name: en.privacy.consent.monitoring.label,
    })
    const clipboard = await screen.findByRole('checkbox', {
      name: en.privacy.consent.clipboard.label,
    })
    const microphone = await screen.findByRole('checkbox', {
      name: en.privacy.consent.microphone.label,
    })
    const ocr = await screen.findByRole('checkbox', {
      name: en.privacy.consent.ocrProcessing.label,
    })
    const fullText = await screen.findByRole('checkbox', {
      name: en.privacy.consent.fullText.label,
    })
    const unredactedExternalOcr = await screen.findByRole('checkbox', {
      name: en.privacy.consent.unredactedExternalOcr.label,
    })
    expect(monitoring).toBeInTheDocument()
    expect(clipboard).toBeInTheDocument()
    expect(microphone).toBeInTheDocument()
    expect(ocr).toBeInTheDocument()
    expect(fullText).toBeInTheDocument()
    expect(unredactedExternalOcr).toBeInTheDocument()
    // Confirm the control found by accessible name is the same node found by testId (not a separate input).
    expect(monitoring).toBe(screen.getByTestId('consent-monitoring-toggle'))
    expect(clipboard).toBe(screen.getByTestId('consent-clipboard-toggle'))
    expect(microphone).toBe(screen.getByTestId('consent-microphone-toggle'))
    expect(ocr).toBe(screen.getByTestId('consent-ocr-processing-toggle'))
    expect(fullText).toBe(screen.getByTestId('consent-full-text-toggle'))
    expect(unredactedExternalOcr).toBe(screen.getByTestId('consent-unredacted-external-ocr-toggle'))
  })

  // F7 (demo/standalone): when Tauri is absent, get_consent rejects → useQuery enters the error state
  // and data stays undefined. It must render an "unavailable" note instead of a perpetual loading card.
  it('renders an unavailable note (not a stuck loading card) when get_consent rejects', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_consent') {
        throw new Error('window.__TAURI_INTERNALS__ is undefined')
      }
      return undefined
    })

    renderWithProviders(<ConsentToggleSection />)

    // The unavailable note is exposed.
    const unavailable = await screen.findByText(en.privacy.consent.unavailable)
    expect(unavailable).toBeInTheDocument()
    // The perpetual loading card must no longer be visible.
    expect(screen.queryByText(en.privacy.consent.loading)).not.toBeInTheDocument()
  })
})
