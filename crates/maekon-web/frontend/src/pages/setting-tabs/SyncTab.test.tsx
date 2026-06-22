import { screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import SyncTab from './SyncTab'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() => Promise.reject(new Error('Tauri unavailable'))),
}))

describe('SyncTab', () => {
  it('falls back to the unavailable sync guide when Tauri sync status is unavailable', async () => {
    renderWithProviders(<SyncTab />)

    await waitFor(() => {
      expect(screen.getByRole('region', { name: 'Sync setup guide' })).toBeInTheDocument()
    })
    expect(screen.getByText('Sync is enabled, but the local sync runtime is not available.')).toBeInTheDocument()
    expect(screen.queryByText('Loading sync status...')).not.toBeInTheDocument()
  })

  it('shows setup steps only when sync is explicitly disabled', async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    vi.mocked(invoke).mockResolvedValueOnce({
      enabled: false,
      runtime_available: false,
      runtime_state: 'disabled',
      unavailable_reason: null,
      device_id: '',
      device_name: '',
    })

    renderWithProviders(<SyncTab />)

    await waitFor(() => {
      expect(screen.getByText('Sync is not enabled. To activate cross-device sync:')).toBeInTheDocument()
    })
  })
})
