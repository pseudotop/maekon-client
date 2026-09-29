import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import TimelineView from './TimelineView'

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}))

const baseEntry = {
  segment_id: 'segment-1',
  start_time: '2026-09-03T00:00:00Z',
  end_time: '2026-09-03T00:30:00Z',
  duration_mins: 30,
  regime_label: 'Focus',
  regime_color: '#123456',
  dominant_app: 'Editor',
  content_summary: [],
}

describe('TimelineView AI provenance (#11738)', () => {
  it('renders generated segment text with its provider class', () => {
    render(
      <TimelineView
        timeline={[
          {
            ...baseEntry,
            ai_summary: {
              text: 'Implemented the summary pipeline',
              provider_class: 'loopback',
              generated_at: '2026-09-03T00:31:00Z',
            },
          },
        ]}
      />,
    )

    expect(screen.getByText('summaryProvenance.aiSegmentSummary')).toBeInTheDocument()
    expect(screen.getByText('summaryProvenance.provider.loopback')).toBeInTheDocument()
    expect(screen.getByText('Implemented the summary pipeline')).toBeInTheDocument()
  })

  it('reveals the safe failure reason only when the segment is expanded', () => {
    render(
      <TimelineView
        timeline={[
          {
            ...baseEntry,
            ai_summary: { provider_class: 'external_api', failure_reason: 'provider_failed' },
          },
        ]}
      />,
    )

    expect(screen.queryByText('summaryProvenance.failure.provider_failed')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button'))
    expect(screen.getByText(/summaryProvenance\.aiSegmentUnavailable/)).toHaveTextContent(
      'summaryProvenance.failure.provider_failed',
    )
  })
})
