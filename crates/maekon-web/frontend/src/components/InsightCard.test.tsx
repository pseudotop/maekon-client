import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import InsightCard from './InsightCard'

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}))

const insight = {
  narrative: 'Legacy or generated narrative',
  highlights: [{ highlight_type: 'ACHIEVEMENT', text: 'Focused block' }],
}

describe('InsightCard AI provenance (#11738)', () => {
  it('does not relabel an unproven legacy narrative as AI', () => {
    render(
      <InsightCard insight={insight} digestProvenance="heuristic" aiNarrative={{ failure_reason: 'not_generated' }} />,
    )

    expect(screen.getByText('summaryProvenance.digest.heuristic')).toBeInTheDocument()
    expect(screen.getByText('summaryProvenance.dailyNarrativeUnavailable')).toBeInTheDocument()
    expect(screen.getByText('summaryProvenance.failure.not_generated')).toBeInTheDocument()
    expect(screen.queryByText('summaryProvenance.aiDailyNarrative')).not.toBeInTheDocument()
    expect(screen.queryByText(insight.narrative)).not.toBeInTheDocument()
  })

  it('renders a persisted AI narrative with provider provenance', () => {
    render(
      <InsightCard
        insight={insight}
        digestProvenance="heuristic"
        aiNarrative={{
          text: 'Provider-backed daily narrative',
          provider_class: 'external_api',
          generated_at: '2026-09-03T00:00:00Z',
        }}
      />,
    )

    expect(screen.getByText('summaryProvenance.digest.heuristic')).toBeInTheDocument()
    expect(screen.getByText(/summaryProvenance\.aiDailyNarrative/)).toBeInTheDocument()
    expect(screen.getByText(/summaryProvenance\.provider\.external_api/)).toBeInTheDocument()
    expect(screen.getByText('Provider-backed daily narrative')).toBeInTheDocument()
    expect(screen.getByText('Focused block')).toBeInTheDocument()
  })
})
