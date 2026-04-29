import { fireEvent, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { fetchCoachingTemplates, fetchPresetLibrary } from '../api/client'
import Playbooks from './Playbooks'

vi.mock('../api/client', () => ({
  fetchCoachingTemplates: vi.fn(),
  fetchPresetLibrary: vi.fn(),
}))

describe('Playbooks page', () => {
  beforeEach(() => {
    vi.mocked(fetchCoachingTemplates).mockResolvedValue({
      templates: [
        {
          profile: 'FocusGuard',
          trigger_type: 'RegimeTransition',
          tone: 'direct',
          locale: 'en',
          text: "You've switched from {regime} - {context_switches} switches in 30 min.",
        },
      ],
    })
    vi.mocked(fetchPresetLibrary).mockResolvedValue({ presets: [] })
  })

  it('renders coaching template variables as readable chips instead of raw braces', async () => {
    renderWithProviders(<Playbooks />)

    expect(await screen.findAllByText('FocusGuard')).not.toHaveLength(0)

    expect(screen.getByText('Template variable: regime')).toBeInTheDocument()
    expect(screen.getByText('Template variable: context_switches')).toBeInTheDocument()
    expect(screen.queryByText(/\{regime\}/)).not.toBeInTheDocument()
  })

  it('explains how to make an empty coaching library useful', async () => {
    vi.mocked(fetchCoachingTemplates).mockResolvedValueOnce({ templates: [] })

    renderWithProviders(<Playbooks />)

    expect(await screen.findByText('No Coaching Templates')).toBeInTheDocument()
    expect(screen.getByText('Pick a trigger')).toBeInTheDocument()
    expect(screen.getByText('Keep variables readable')).toBeInTheDocument()
    expect(screen.getByText('Preview before enabling')).toBeInTheDocument()
  })

  it('explains how to make an empty preset library useful', async () => {
    vi.mocked(fetchPresetLibrary).mockResolvedValueOnce({ presets: [] })

    renderWithProviders(<Playbooks />)

    fireEvent.click(screen.getByRole('button', { name: /Automation Presets/i }))

    expect(await screen.findByText('No Automation Presets')).toBeInTheDocument()
    expect(screen.getByText('Review trusted commands')).toBeInTheDocument()
    expect(screen.getByText('Bind policies first')).toBeInTheDocument()
    expect(screen.getByText('Run from Commands')).toBeInTheDocument()
  })
})
