import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { fireEvent, screen, waitFor } from '@testing-library/react'
import { Route, Routes } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { StorageStats } from '../../../api/contracts'
import DataSection from '../DataSection'
import PrivacyLayout from '../PrivacyLayout'

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
  }
})

function renderPrivacyRoute(initialEntry: string) {
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
      </Route>
    </Routes>,
    { routerProps: { initialEntries: [initialEntry] } },
  )
  return { ...rendered, getCaptureStatusSpy }
}

afterEach(() => {
  clearMocks()
})

describe('PrivacyLayout routed content placement', () => {
  it('keeps consent controls on the data overview', async () => {
    renderPrivacyRoute('/privacy/data')

    expect(await screen.findByTestId('consent-toggle-controls')).toBeInTheDocument()
  })

  it('does not place consent controls before the selected egress route', async () => {
    renderPrivacyRoute('/privacy/egress')

    expect(await screen.findByTestId('egress-route-content')).toBeInTheDocument()
    expect(screen.queryByTestId('consent-toggle-controls')).not.toBeInTheDocument()
  })

  it('refreshes capture state after the data consent controls report a change', async () => {
    const { getCaptureStatusSpy } = renderPrivacyRoute('/privacy/data')
    const consentControls = await screen.findByTestId('consent-toggle-controls')
    await waitFor(() => expect(getCaptureStatusSpy).toHaveBeenCalledTimes(1))

    fireEvent.click(consentControls)

    await waitFor(() => expect(getCaptureStatusSpy).toHaveBeenCalledTimes(2))
  })
})
