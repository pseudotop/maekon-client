/**
 * Delete-all danger zone behavior test (#4629 A.2 task 5).
 *
 * design.md §2 / §0.1.d: "delete all data" *must* call withdrawConsent() first,
 * synchronously nulling the in-memory current_consent (fail-closed), and only then
 * perform the existing deleteAllData() REST deletion (revoke-first, race-free). If
 * withdrawConsent() throws, do not proceed with deletion (no silent delete without revocation).
 *
 * This test drives PrivacyLayout's real deleteAllMutation as-is (rendering ConsentSection as a
 * child route) — i.e. it verifies the production mutation wiring. It wraps client.ts's
 * withdrawConsent / deleteAllData in spies to confirm the call *order* and the reject short-circuit.
 * (deleteAllData uses fetch and withdrawConsent uses Tauri IPC, but the ordering assertion that
 * spans both transport layers is observed deterministically at the single client.ts function spy.)
 */

import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { Route, Routes } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { DeleteResult, StorageStats } from '../../../api/contracts'
import * as toastModule from '../../../hooks/useToast'
import en from '../../../i18n/locales/en.json'
import ConsentSection from '../ConsentSection'
import PrivacyLayout from '../PrivacyLayout'

// The vi.mock factory is hoisted to the top of the file, so the spies/shared sequence it
// references are lifted via vi.hoisted() to match initialization order (preventing a TDZ error
// when referencing a top-level const).
const { callOrder, withdrawSpy, deleteAllSpy } = vi.hoisted(() => {
  const order: string[] = []
  return {
    callOrder: order,
    withdrawSpy: vi.fn(async () => {
      order.push('withdraw')
      return { status: 'NotGranted', permissions: {} } as never
    }),
    deleteAllSpy: vi.fn(async (): Promise<DeleteResult> => {
      order.push('delete')
      return {
        success: true,
        message: 'deleted',
        events_deleted: 0,
        frames_deleted: 0,
        metrics_deleted: 0,
        process_snapshots_deleted: 0,
        idle_periods_deleted: 0,
      }
    }),
  }
})

// Keep client.ts's original, but replace only deleteAllData / withdrawConsent / fetchStorageStats.
// Stub fetchStorageStats so the layout passes its isLoading gate (avoiding the network).
vi.mock('../../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../api/client')>()
  const stats: StorageStats = {
    db_size_bytes: 0,
    frames_size_bytes: 0,
    total_size_bytes: 0,
    frame_count: 0,
    event_count: 0,
    metric_count: 0,
    oldest_data_date: null,
    newest_data_date: null,
  }
  return {
    ...actual,
    fetchStorageStats: vi.fn(async () => stats),
    deleteAllData: deleteAllSpy,
    withdrawConsent: withdrawSpy,
  }
})

/**
 * Renders PrivacyLayout (parent, owning the real deleteAllMutation) + ConsentSection (child index
 * route, danger button + context consumer) in a memory router. The get_capture_status IPC is
 * answered via mockIPC so the layout's mount effect does not fall into reject.
 */
function renderDangerZone(
  captureStatus = { paused: false, indicator_visible: true, consent_granted: true, permitted: true },
) {
  mockIPC((cmd) => {
    if (cmd === 'get_capture_status') {
      return captureStatus
    }
    return undefined
  })
  return renderWithProviders(
    <Routes>
      <Route path="/" element={<PrivacyLayout />}>
        <Route index element={<ConsentSection />} />
      </Route>
    </Routes>,
  )
}

/** Click the danger button → click ConfirmModal's "Delete All Data" confirm button. */
async function triggerDeleteAll() {
  const trigger = await screen.findByTestId('delete-all')
  fireEvent.click(trigger)
  // The trigger button and the modal's confirm button use the same copy (deleteAllButton), so
  // narrow the scope to inside the alertdialog to find only the modal's confirm (danger) button.
  const dialog = await screen.findByRole('alertdialog')
  const confirm = within(dialog).getByRole('button', { name: en.privacy.deleteAllButton })
  fireEvent.click(confirm)
}

describe('ConsentSection delete-all (revoke-first)', () => {
  afterEach(() => {
    clearMocks()
    callOrder.length = 0
    withdrawSpy.mockClear()
    deleteAllSpy.mockClear()
    vi.restoreAllMocks()
  })

  it('shows consent-required state and disables pause before screen consent', async () => {
    renderDangerZone({
      paused: false,
      indicator_visible: true,
      consent_granted: false,
      permitted: false,
    })

    expect((await screen.findAllByText(en.privacy.captureConsentRequired)).length).toBeGreaterThan(0)
    expect(screen.queryByText(en.privacy.captureRunning)).not.toBeInTheDocument()
    expect(screen.getByRole('switch', { name: en.privacy.captureConsentRequired })).toBeDisabled()
  })
  it('calls withdraw_consent BEFORE deleteAllData (revoke-first ordering)', async () => {
    renderDangerZone()

    await triggerDeleteAll()

    await waitFor(() => expect(deleteAllSpy).toHaveBeenCalledTimes(1))
    expect(withdrawSpy).toHaveBeenCalledTimes(1)
    // Order: withdraw must be at an earlier index than delete.
    expect(callOrder).toEqual(['withdraw', 'delete'])
    expect(callOrder.indexOf('withdraw')).toBeLessThan(callOrder.indexOf('delete'))
  })

  it('does NOT call deleteAllData and surfaces the error when withdrawConsent rejects', async () => {
    const addToastSpy = vi.spyOn(toastModule, 'addToast')
    withdrawSpy.mockImplementationOnce(async () => {
      callOrder.push('withdraw')
      throw new Error('withdraw failed')
    })

    renderDangerZone()

    await triggerDeleteAll()

    // Wait until the error surfaces (mutation onError → addToast('error', ...)).
    await waitFor(() => expect(addToastSpy).toHaveBeenCalledWith('error', 'withdraw failed'))
    // Since revocation failed, do not proceed with deletion (short-circuit).
    expect(deleteAllSpy).not.toHaveBeenCalled()
    expect(callOrder).toEqual(['withdraw'])
  })
})
