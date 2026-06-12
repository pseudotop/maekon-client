import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import SuggestionBanner from './SuggestionBanner'

const mockFetchUnifiedSuggestions = vi.fn()
const mockSubmitUnifiedSuggestionFeedback = vi.fn()

vi.mock('../api/client', async () => {
  const actual = await vi.importActual<typeof import('../api/client')>('../api/client')
  return {
    ...actual,
    fetchUnifiedSuggestions: (...args: unknown[]) => mockFetchUnifiedSuggestions(...args),
    submitUnifiedSuggestionFeedback: (...args: unknown[]) => mockSubmitUnifiedSuggestionFeedback(...args),
  }
})

describe('SuggestionBanner', () => {
  beforeEach(() => {
    mockFetchUnifiedSuggestions.mockReset()
    mockSubmitUnifiedSuggestionFeedback.mockReset()
  })

  it('labels the non-executing acted feedback button as an acknowledgement', async () => {
    mockFetchUnifiedSuggestions.mockResolvedValue([
      {
        id: 1,
        suggestion_id: 'sug-1',
        suggestion_type: 'NeedFocusTime',
        source: 'local',
        content: 'Protect the next 25 minutes for focused work.',
        priority: 'medium',
        confidence_score: 0.9,
        relevance_score: 0.8,
        is_actionable: true,
        shown_at: null,
        dismissed_at: null,
        acted_at: null,
        created_at: '2026-05-28T01:20:00Z',
        expires_at: null,
      },
    ])
    mockSubmitUnifiedSuggestionFeedback.mockResolvedValue(undefined)

    renderWithProviders(<SuggestionBanner />)

    const acknowledge = await screen.findByRole('button', { name: /mark done/i })
    expect(screen.queryByRole('button', { name: /^act$/i })).not.toBeInTheDocument()

    fireEvent.click(acknowledge)

    await waitFor(() => {
      expect(mockSubmitUnifiedSuggestionFeedback).toHaveBeenCalledWith('sug-1', 'acted')
    })
  })
})
