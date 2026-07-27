import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { ExtensionRegistryPanel } from './ExtensionRegistryPanel'

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

const baseExtension = {
  install_id: 'inst_1',
  extension_id: 'com.maekon.calendar',
  version: '1.0.0',
  provenance: 'BUNDLED' as const,
  availability: { state: 'available' as const },
  installation: 'installed' as const,
  enablement: 'enabled' as const,
  authentication: 'not_required' as const,
  grant: 'granted' as const,
  update: 'current' as const,
  health: { state: 'healthy' as const },
  previous_version: null,
  revision: 3,
  summary_label: 'ready',
}

function primeList(extensions: unknown[]) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'list_extensions') return Promise.resolve(extensions)
    return Promise.resolve({ outcome: 'applied' })
  })
}

describe('ExtensionRegistryPanel', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
  })

  it('renders every readiness axis alongside the summary', async () => {
    primeList([baseExtension])
    renderWithProviders(<ExtensionRegistryPanel />)

    await waitFor(() => {
      expect(screen.getByText('com.maekon.calendar')).toBeInTheDocument()
    })
    expect(screen.getByTestId('summary-inst_1')).toHaveTextContent('Ready')
    // All six visible axes are present — none collapsed away.
    expect(screen.getByText(/Installation: Installed/)).toBeInTheDocument()
    expect(screen.getByText(/Enablement: Enabled/)).toBeInTheDocument()
    expect(screen.getByText(/Account: Not required/)).toBeInTheDocument()
    expect(screen.getByText(/Capability grant: Granted/)).toBeInTheDocument()
    expect(screen.getByText(/Update: Current/)).toBeInTheDocument()
    expect(screen.getByText(/Health: Healthy/)).toBeInTheDocument()
  })

  it('shows the axes and no summary while a transition is in flight', async () => {
    primeList([{ ...baseExtension, installation: 'installing', summary_label: null }])
    renderWithProviders(<ExtensionRegistryPanel />)

    await waitFor(() => {
      expect(screen.getByText('In transition — see details')).toBeInTheDocument()
    })
    expect(screen.queryByTestId('summary-inst_1')).not.toBeInTheDocument()
    expect(screen.getByText(/Installation: Installing/)).toBeInTheDocument()
  })

  it('surfaces an unavailable reason and offers no install action', async () => {
    primeList([
      {
        ...baseExtension,
        installation: 'not_installed',
        summary_label: 'incompatible',
        availability: { state: 'unavailable', reason: 'execution_location_unsupported' },
      },
    ])
    renderWithProviders(<ExtensionRegistryPanel />)

    await waitFor(() => {
      expect(screen.getByText(/Unavailable: execution_location_unsupported/)).toBeInTheDocument()
    })
    // An unavailable package must never offer Install.
    expect(screen.queryByRole('button', { name: 'Install' })).not.toBeInTheDocument()
  })

  it('sends the current revision when toggling enablement', async () => {
    primeList([baseExtension])
    renderWithProviders(<ExtensionRegistryPanel />)

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Disable' })).toBeInTheDocument()
    })
    fireEvent.click(screen.getByRole('button', { name: 'Disable' }))

    await waitFor(() => {
      const call = mockInvoke.mock.calls.find((c) => c[0] === 'set_extension_enablement')
      expect(call).toBeDefined()
      const args = call?.[1] as Record<string, unknown>
      expect(args.installId).toBe('inst_1')
      expect(args.enabled).toBe(false)
      expect(args.expectedRevision).toBe(3)
    })
  })

  it('offers rollback only when a previous known-good version exists', async () => {
    primeList([{ ...baseExtension, previous_version: '0.9.0' }])
    renderWithProviders(<ExtensionRegistryPanel />)

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Roll back to 0.9.0' })).toBeInTheDocument()
    })
  })

  it('shows an empty state rather than a marketplace', async () => {
    primeList([])
    renderWithProviders(<ExtensionRegistryPanel />)

    await waitFor(() => {
      expect(screen.getByText('No extensions are registered.')).toBeInTheDocument()
    })
  })
})
