import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import TimelineView from './TimelineView'

// Keys come back as-is; interpolation values are appended so a test can see what was passed.
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, values?: Record<string, unknown>) => (values ? `${key} ${JSON.stringify(values)}` : key),
  }),
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

// #13740: segment lengths read like every other duration instead of a raw minute count.
describe('TimelineView durations', () => {
  it('shows segment and content lengths with the shared duration format', () => {
    render(
      <TimelineView
        timeline={[
          {
            ...baseEntry,
            duration_mins: 135,
            content_summary: [{ work_type: 'ACTIVE_CODING', mins: 45, content: 'parser' }],
          },
        ]}
      />,
    )

    expect(screen.getByText('2h 15m')).toBeInTheDocument()
    expect(screen.getByText('workTypes.ACTIVE_CODING (45m)')).toBeInTheDocument()
  })
})

// #13738: the expand hint and work type names go through translation keys instead of English text.
describe('TimelineView labels', () => {
  it('builds the expand hint from a key with the shared duration format', () => {
    render(<TimelineView timeline={[{ ...baseEntry, duration_mins: 135 }]} />)

    expect(
      screen.getByRole('button', { name: 'timeline.entryExpandLabel {"regime":"Focus","duration":"2h 15m"}' }),
    ).toBeInTheDocument()
  })

  it('names a work type this build does not know in plain words', () => {
    render(
      <TimelineView
        timeline={[{ ...baseEntry, content_summary: [{ work_type: 'SPATIAL_DESIGN', mins: 5, content: 'canvas' }] }]}
      />,
    )

    expect(screen.getByText('Spatial Design (5m)')).toBeInTheDocument()
  })
})
