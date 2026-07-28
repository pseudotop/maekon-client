import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import SyncTab from './SyncTab'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() => Promise.reject(new Error('Tauri unavailable'))),
}))

describe('SyncTab', () => {
  beforeEach(async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    vi.mocked(invoke).mockReset()
    vi.mocked(invoke).mockRejectedValue(new Error('Tauri unavailable'))
  })

  it('falls back to the unavailable sync guide when Tauri sync status is unavailable', async () => {
    renderWithProviders(<SyncTab />)

    await waitFor(() => {
      expect(screen.getByRole('region', { name: 'Sync setup guide' })).toBeInTheDocument()
    })
    expect(screen.getByText('Sync is enabled, but the local sync runtime is not available.')).toBeInTheDocument()
    expect(screen.queryByText('Loading sync status...')).not.toBeInTheDocument()
  })

  it('shows the keychain-backed enable form when sync is explicitly disabled', async () => {
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

    // #8056 P2-2: the disabled state now offers an in-app enable/passphrase form
    // (keychain-backed) instead of manual config-file + env-var instructions.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Enable sync' })).toBeInTheDocument()
    })
    expect(screen.getByLabelText('Sync passphrase')).toBeInTheDocument()
  })

  it('requires confirmation before disconnecting a discovered peer', async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === 'get_sync_status') {
        return Promise.resolve({
          enabled: true,
          runtime_available: true,
          runtime_state: 'ready',
          unavailable_reason: null,
          device_id: 'qc-local-device',
          device_name: 'Synthetic local device',
        })
      }
      if (command === 'discover_sync_peers') {
        return Promise.resolve([
          {
            device_id: 'qc-peer-cj-05-04',
            device_name: 'Synthetic recovery peer',
            last_sync_at: '2026-07-19T00:00:00Z',
          },
        ])
      }
      if (command === 'get_qc_upload_spool_status') return Promise.resolve(null)
      if (command === 'forget_sync_peer') return Promise.resolve([])
      return Promise.reject(new Error('Unexpected command'))
    })

    renderWithProviders(<SyncTab />)

    expect(await screen.findByText('Synthetic recovery peer')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect' }))
    expect(screen.getByRole('alertdialog')).toBeInTheDocument()
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('forget_sync_peer', expect.anything())

    fireEvent.click(screen.getByRole('button', { name: 'Disconnect peer' }))

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('forget_sync_peer', {
        deviceId: 'qc-peer-cj-05-04',
      })
    })
    expect(await screen.findByRole('status')).toHaveTextContent('Synthetic recovery peer was disconnected.')
    expect(screen.queryByText('Synthetic recovery peer')).not.toBeInTheDocument()
  })

  it('leaves the peer connected when confirmation is cancelled', async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === 'get_sync_status') {
        return Promise.resolve({
          enabled: true,
          runtime_available: true,
          runtime_state: 'ready',
          unavailable_reason: null,
          device_id: 'qc-local-device',
          device_name: 'Synthetic local device',
        })
      }
      if (command === 'discover_sync_peers') {
        return Promise.resolve([
          {
            device_id: 'qc-peer-cj-05-04',
            device_name: 'Synthetic recovery peer',
            last_sync_at: '2026-07-19T00:00:00Z',
          },
        ])
      }
      if (command === 'get_qc_upload_spool_status') return Promise.resolve(null)
      return Promise.reject(new Error('Unexpected command'))
    })

    renderWithProviders(<SyncTab />)

    expect(await screen.findByText('Synthetic recovery peer')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect' }))
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
    expect(screen.getByText('Synthetic recovery peer')).toBeInTheDocument()
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('forget_sync_peer', expect.anything())
  })

  it('shows the synthetic upload interruption and exact retry recovery journey', async () => {
    const { invoke } = await import('@tauri-apps/api/core')
    let phase: 'interrupted' | 'recovered' = 'interrupted'
    const spoolStatus = () => ({
      phase,
      pending_count: phase === 'recovered' ? 0 : 2,
      attempt_count: phase === 'interrupted' ? 1 : 2,
      reprime_count: phase === 'interrupted' ? 1 : 2,
      sent_marker_count: phase === 'recovered' ? 2 : 0,
      storage_id_preserved: true,
      network_disabled: true,
      real_account_used: false,
      last_error: phase === 'interrupted' ? 'synthetic_transport_interrupted' : null,
    })
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === 'get_sync_status') {
        return Promise.resolve({
          enabled: true,
          runtime_available: true,
          runtime_state: 'ready',
          unavailable_reason: null,
          device_id: 'qc-local-device',
          device_name: 'Synthetic local device',
        })
      }
      if (command === 'discover_sync_peers') return Promise.resolve([])
      if (command === 'get_qc_upload_spool_status') return Promise.resolve(spoolStatus())
      if (command === 'run_qc_upload_spool_step') {
        phase = 'recovered'
        return Promise.resolve(spoolStatus())
      }
      return Promise.reject(new Error('Unexpected command'))
    })

    renderWithProviders(<SyncTab />)

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('get_qc_upload_spool_status', undefined)
    })
    const panel = await screen.findByRole('region', { name: 'Upload recovery check' })
    expect(await screen.findByRole('alert')).toHaveTextContent('2 synthetic items remain local and unsent')
    expect(panel).toHaveTextContent('Sent markers0')

    fireEvent.click(screen.getByRole('button', { name: 'Retry safely' }))
    const recovered = await screen.findByText(/Upload recovered after 2 attempts and 2 re-primes/)
    expect(recovered).toHaveAttribute('role', 'status')
    expect(panel).toHaveTextContent('Pending0')
    expect(panel).toHaveTextContent('Sent markers2')
    expect(panel).toHaveTextContent('Preserved')
  }, 15_000)
})
