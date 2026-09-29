import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { fireEvent, screen, waitFor } from '@testing-library/react'
import { Route, Routes, useOutletContext } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { BackupArchive, RestoreResult, StorageStats } from '../../../api/contracts'
import { aiCapabilityReadiness, aiReadinessActionRoute } from '../../../features/aiReadiness'
import DataSection from '../DataSection'
import PrivacyLayout, { type PrivacyContext } from '../PrivacyLayout'

const mocks = vi.hoisted(() => ({
  invalidateReadiness: vi.fn(),
  restoreBackup: vi.fn(),
}))

vi.mock('../../../hooks/useAiReadinessSnapshot', () => ({
  invalidateAiReadinessSnapshotCache: mocks.invalidateReadiness,
}))

vi.mock('../ConsentToggleSection', () => ({
  default: ({ onConsentChanged }: { onConsentChanged?: () => void }) => (
    <button type="button" data-testid="consent-toggle-controls" onClick={onConsentChanged}>
      consent controls
    </button>
  ),
}))

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
    restoreBackup: mocks.restoreBackup,
  }
})

function RestoreHarness() {
  const { restoreMutation } = useOutletContext<PrivacyContext>()
  return (
    <button type="button" onClick={() => restoreMutation.mutate({} as BackupArchive)}>
      restore settings
    </button>
  )
}

/**
 * Renders the privacy routes and waits for PrivacyLayout's one mount-time
 * `get_capture_status` read. That read runs after a dynamic import, so under
 * load it could finish after the test that caused it, landing in the next
 * test's IPC handler and inflating its exact call count (#12517). Waiting
 * here keeps each read inside the test that made it.
 */
async function renderPrivacyRoute(initialEntry: string) {
  const getCaptureStatusSpy = vi.fn(() => ({
    paused: false,
    indicator_visible: true,
    consent_granted: false,
    permitted: false,
  }))
  mockIPC((command) => {
    if (command === 'get_capture_status') {
      return getCaptureStatusSpy()
    }
    return undefined
  })

  const rendered = renderWithProviders(
    <Routes>
      <Route path="/privacy" element={<PrivacyLayout />}>
        <Route path="data" element={<DataSection />} />
        <Route path="egress" element={<div data-testid="egress-route-content" />} />
        <Route path="restore" element={<RestoreHarness />} />
      </Route>
    </Routes>,
    { routerProps: { initialEntries: [initialEntry] } },
  )
  await waitFor(() => expect(getCaptureStatusSpy).toHaveBeenCalledTimes(1))
  return { ...rendered, getCaptureStatusSpy }
}

afterEach(() => {
  clearMocks()
  mocks.invalidateReadiness.mockReset()
  mocks.restoreBackup.mockReset()
})

describe('PrivacyLayout routed content placement', () => {
  it('routes AI consent recovery to the mounted consent controls', async () => {
    const readiness = aiCapabilityReadiness(undefined, 'chat.subprocess')
    const route = aiReadinessActionRoute({ ...readiness, action: 'open_privacy_consent' })
    expect(route).not.toBeNull()
    await renderPrivacyRoute(route as string)

    expect(await screen.findByTestId('consent-toggle-controls')).toBeInTheDocument()
  })

  it('keeps consent controls on the data overview', async () => {
    await renderPrivacyRoute('/privacy/data')

    expect(await screen.findByTestId('consent-toggle-controls')).toBeInTheDocument()
  })

  it('does not place consent controls before the selected egress route', async () => {
    await renderPrivacyRoute('/privacy/egress')

    expect(await screen.findByTestId('egress-route-content')).toBeInTheDocument()
    expect(screen.queryByTestId('consent-toggle-controls')).not.toBeInTheDocument()
  })

  it('refreshes capture state after the data consent controls report a change', async () => {
    const { getCaptureStatusSpy } = await renderPrivacyRoute('/privacy/data')
    const consentControls = await screen.findByTestId('consent-toggle-controls')
    expect(getCaptureStatusSpy).toHaveBeenCalledTimes(1)

    fireEvent.click(consentControls)

    await waitFor(() => expect(getCaptureStatusSpy).toHaveBeenCalledTimes(2))
  })

  it('invalidates AI readiness after a settings restore succeeds', async () => {
    const result: RestoreResult = {
      success: true,
      restored: { settings: true, tags: 0, frame_tags: 0, events: 0, frames: 0 },
      errors: [],
    }
    mocks.restoreBackup.mockResolvedValue(result)
    await renderPrivacyRoute('/privacy/restore')

    fireEvent.click(await screen.findByRole('button', { name: 'restore settings' }))

    await waitFor(() => expect(mocks.restoreBackup).toHaveBeenCalledOnce())
    expect(mocks.invalidateReadiness).toHaveBeenCalledOnce()
  })
})
