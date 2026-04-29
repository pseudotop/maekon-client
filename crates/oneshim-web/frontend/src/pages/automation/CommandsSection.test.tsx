import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { fetchPresets, fetchSettings } from '../../api/client'
import CommandsSection from './CommandsSection'

const mockUseTypedOutletContext = vi.hoisted(() => vi.fn())

vi.mock('../../routes', () => ({
  useTypedOutletContext: mockUseTypedOutletContext,
}))

vi.mock('../../api/client', () => ({
  deletePreset: vi.fn(),
  fetchPresets: vi.fn(),
  fetchSettings: vi.fn(),
  runPreset: vi.fn(),
  updatePreset: vi.fn(),
}))

describe('Automation CommandsSection', () => {
  beforeEach(() => {
    mockUseTypedOutletContext.mockReturnValue({ status: { enabled: false } })
    vi.mocked(fetchPresets).mockResolvedValue({ presets: [] })
    vi.mocked(fetchSettings).mockResolvedValue({ ai_provider: { saved_profiles: [] } } as never)
  })

  it('explains how to make an empty preset category useful', async () => {
    renderWithProviders(<CommandsSection />)

    await waitFor(() => {
      expect(screen.getByText('No presets in this category')).toBeInTheDocument()
    })

    expect(screen.getByText('Check another category')).toBeInTheDocument()
    expect(screen.getByText('Bind AI profiles later')).toBeInTheDocument()
    expect(screen.getByText('Run only after policy setup')).toBeInTheDocument()
  })
})
