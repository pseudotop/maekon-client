import { fireEvent, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import SupportToolsCard from './SupportToolsCard'

vi.mock('../api/client', () => ({
  fetchSupportDiagnostics: vi.fn().mockResolvedValue({
    schema_version: 'support.diagnostics.v1',
    generated_at: '2026-07-12T00:00:00Z',
    health: {
      storage_ok: true,
      frames_dir_configured: true,
      frames_dir_path: 'C:\\Users\\alice\\AppData\\Local\\maekon\\data',
      frames_dir_exists: true,
      config_manager_configured: true,
      automation_controller_configured: true,
      update_control_configured: true,
    },
    provider_cli: [],
    recent_audit_entries: [],
    recent_policy_events: [],
  }),
}))

describe('SupportToolsCard privacy', () => {
  it('opens the canonical Alpha feedback and privacy route', () => {
    const openSpy = vi.spyOn(window, 'open').mockReturnValue({} as Window)
    renderWithProviders(<SupportToolsCard />)

    fireEvent.click(screen.getByRole('button', { name: 'Alpha feedback & privacy' }))

    expect(openSpy).toHaveBeenCalledWith('https://maekon.dev/alpha-feedback', '_blank', 'noopener,noreferrer')
    openSpy.mockRestore()
  })

  it('shows useful frames-directory status without rendering the local path', async () => {
    renderWithProviders(<SupportToolsCard />)

    fireEvent.click(screen.getByRole('button', { name: 'Open Support Details' }))

    await waitFor(() => {
      expect(screen.getByText('support.diagnostics.v1')).toBeInTheDocument()
    })

    expect(screen.getByText('Frames directory')).toBeInTheDocument()
    expect(screen.queryByText(/C:\\Users\\alice/i)).not.toBeInTheDocument()
    expect(screen.getByText('Yes · Yes')).toBeInTheDocument()
  })
})
