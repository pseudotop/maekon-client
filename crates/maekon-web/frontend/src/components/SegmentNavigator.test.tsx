import { fireEvent, render, screen } from '@testing-library/react'
import '@testing-library/jest-dom/vitest'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import type { AppSegment } from '../api/contracts'
import { SegmentNavigator } from './SegmentNavigator'

// The bare `render` case below goes through this stub: `useTranslation` with no
// `i18n` object, which is exactly what broke the reauth journey test.
vi.mock('react-i18next', async (importOriginal) => {
  const mod = await importOriginal<typeof import('react-i18next')>()
  return {
    ...mod,
    useTranslation: () => ({
      t: (key: string, fallback?: string) => fallback ?? key,
      i18n: undefined,
    }),
  }
})

const SEGMENTS: AppSegment[] = [
  { app_name: 'Code', start: '2026-08-01T09:00:00Z', end: '2026-08-01T09:30:00Z', color: '#3b82f6' },
  { app_name: 'Chrome', start: '2026-08-01T09:30:00Z', end: '2026-08-01T10:00:00Z', color: '#ef4444' },
  // A 1-second segment. Without the floor this is 0.027% of the hour-long
  // span — a sliver no pointer can hit.
  { app_name: 'Slack', start: '2026-08-01T10:00:00Z', end: '2026-08-01T10:00:01Z', color: '#22c55e' },
]

describe('SegmentNavigator (#9812)', () => {
  it('renders one band per segment', () => {
    renderWithProviders(<SegmentNavigator segments={SEGMENTS} selectedStart={null} onSelect={vi.fn()} />)
    expect(screen.getAllByTestId('segment-band')).toHaveLength(3)
  })

  it('keeps a very short segment clickable instead of collapsing it', () => {
    // 1s out of a ~60m span is 0.027% — visually gone and impossible to click.
    // The floor is what makes "the app I only touched for a moment" reachable.
    // Pinned with a value that actually falls below it: at 30s (0.83%) this
    // test passes with or without the floor, which proves nothing.
    renderWithProviders(<SegmentNavigator segments={SEGMENTS} selectedStart={null} onSelect={vi.fn()} />)
    const slack = screen
      .getAllByTestId('segment-band')
      .find((b) => b.getAttribute('data-segment-start') === '2026-08-01T10:00:00Z')
    const width = Number.parseFloat((slack as HTMLElement).style.width)
    expect(width).toBeGreaterThanOrEqual(0.5)
  })

  it('reports the clicked segment so the caller can narrow the frame query', () => {
    const onSelect = vi.fn()
    renderWithProviders(<SegmentNavigator segments={SEGMENTS} selectedStart={null} onSelect={onSelect} />)

    fireEvent.click(screen.getAllByTestId('segment-band')[1])
    expect(onSelect).toHaveBeenCalledWith(SEGMENTS[1])
  })

  it('clicking the active segment clears the drill-down', () => {
    // Without this the only way back to the full range is the date picker,
    // which reads as "the filter is stuck".
    const onSelect = vi.fn()
    renderWithProviders(<SegmentNavigator segments={SEGMENTS} selectedStart={SEGMENTS[0].start} onSelect={onSelect} />)

    fireEvent.click(screen.getAllByTestId('segment-band')[0])
    expect(onSelect).toHaveBeenCalledWith(null)
  })

  it('marks the active segment for assistive tech, not just visually', () => {
    renderWithProviders(<SegmentNavigator segments={SEGMENTS} selectedStart={SEGMENTS[1].start} onSelect={vi.fn()} />)
    const bands = screen.getAllByTestId('segment-band')
    expect(bands[1]).toHaveAttribute('aria-pressed', 'true')
    expect(bands[0]).toHaveAttribute('aria-pressed', 'false')
  })

  it('survives a host without a full i18n instance', () => {
    // Caught for real: the reauth journey test mounts the timeline with a
    // stubbed `useTranslation`, and reading `i18n.language` unguarded threw
    // there — taking the whole page down. A navigation aid must not be able to
    // break the screen it sits on. Rendering through the same stub shape here
    // keeps that guard honest.
    render(<SegmentNavigator segments={SEGMENTS} selectedStart={null} onSelect={vi.fn()} />)
    expect(screen.getAllByTestId('segment-band')).toHaveLength(3)
  })

  it('explains an empty range rather than rendering a blank strip', () => {
    // Segments are written on a periodic checkpoint, so a just-picked recent
    // range is legitimately empty. A blank axis there looks broken.
    renderWithProviders(<SegmentNavigator segments={[]} selectedStart={null} onSelect={vi.fn()} />)
    expect(screen.getByTestId('segment-navigator-empty')).toBeInTheDocument()
    expect(screen.queryAllByTestId('segment-band')).toHaveLength(0)
  })

  it('lists each app once in the legend even when it appears in several segments', () => {
    const repeated: AppSegment[] = [
      ...SEGMENTS,
      { app_name: 'Code', start: '2026-08-01T10:01:00Z', end: '2026-08-01T10:20:00Z', color: '#3b82f6' },
    ]
    renderWithProviders(<SegmentNavigator segments={repeated} selectedStart={null} onSelect={vi.fn()} />)
    expect(screen.getAllByTestId('segment-band')).toHaveLength(4)
    // `Code` appears in the legend once; the band `sr-only` labels account for
    // the rest, so count only the legend occurrences.
    expect(screen.getAllByText('Code')).toHaveLength(3)
  })
})
