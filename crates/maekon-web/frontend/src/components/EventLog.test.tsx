/**
 * EventLog incremental rendering (#9633).
 *
 * A session easily reaches 1,000+ items; the log used to render every row at
 * once, flattening the page into an endless scroll. These tests pin the two
 * halves of the fix: rows render in chunks behind a "Show more" control, and
 * the window auto-extends to the active row so playback can jump anywhere.
 */

import { fireEvent, render, screen } from '@testing-library/react'
import { beforeAll, describe, expect, it, vi } from 'vitest'
import type { TimelineItem } from '../api/contracts'
import EventLog from './EventLog'

beforeAll(() => {
  // jsdom does not implement scrollIntoView (used to follow the active row).
  Element.prototype.scrollIntoView = vi.fn()
})

function makeItems(count: number): TimelineItem[] {
  return Array.from({ length: count }, (_, i) => ({
    type: 'Frame' as const,
    id: i,
    timestamp: new Date(Date.UTC(2026, 6, 30, 10, 0, i)).toISOString(),
    app_name: `App ${i}`,
    window_title: `window ${i}`,
    importance: 0.5,
    image_url: `/api/frames/${i}/image`,
    ocr_text: null,
  }))
}

describe('EventLog incremental rendering (#9633)', () => {
  it('renders only the first chunk of a large list, with a Show more control', () => {
    const items = makeItems(450)
    render(<EventLog items={items} currentTime={new Date(Date.UTC(2026, 6, 30, 10, 0, 0))} onItemClick={vi.fn()} />)

    // First chunk (200) rendered; the tail is not in the DOM.
    expect(screen.getByText('App 0')).toBeInTheDocument()
    expect(screen.getByText('App 199')).toBeInTheDocument()
    expect(screen.queryByText('App 200')).not.toBeInTheDocument()

    // Show more reveals the next chunk and reports the remainder.
    const showMore = screen.getByRole('button', { name: /Show more/ })
    expect(showMore).toHaveTextContent('250')
    fireEvent.click(showMore)
    expect(screen.getByText('App 399')).toBeInTheDocument()
    expect(screen.queryByText('App 400')).not.toBeInTheDocument()
  })

  it('auto-extends the rendered window to the active playback row', () => {
    const items = makeItems(450)
    // currentTime points at item 420 — beyond the initial 200-row window.
    render(<EventLog items={items} currentTime={new Date(Date.UTC(2026, 6, 30, 10, 0, 420))} onItemClick={vi.fn()} />)

    expect(screen.getByText('App 420')).toBeInTheDocument()
  })

  it('renders small lists without a Show more control', () => {
    render(
      <EventLog items={makeItems(5)} currentTime={new Date(Date.UTC(2026, 6, 30, 10, 0, 0))} onItemClick={vi.fn()} />,
    )
    expect(screen.queryByRole('button', { name: /Show more/ })).not.toBeInTheDocument()
  })
})
