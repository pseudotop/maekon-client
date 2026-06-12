import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { PointerContextPayload } from '../types'
import { normalizePointerContextPayload, PointerContextHighlight } from './PointerContextHighlight'

function payload(overrides: Partial<PointerContextPayload> = {}): PointerContextPayload {
  return {
    enabled: true,
    x: 48,
    y: 72,
    click_count: 1,
    click_pulse: false,
    reduced_motion: false,
    ttl_ms: 900,
    sample_rate_hz: 30,
    ...overrides,
  }
}

describe('PointerContextHighlight', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 100 })
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: 80 })
  })

  afterEach(() => {
    vi.runOnlyPendingTimers()
    vi.useRealTimers()
  })

  it('does not render without an enabled pointer payload', () => {
    const { rerender } = render(<PointerContextHighlight pointerContext={null} />)
    expect(screen.queryByTestId('pointer-context-highlight')).not.toBeInTheDocument()

    rerender(<PointerContextHighlight pointerContext={payload({ enabled: false })} />)
    expect(screen.queryByTestId('pointer-context-highlight')).not.toBeInTheDocument()
  })

  it('ignores missing or non-finite pointer coordinates', () => {
    expect(normalizePointerContextPayload(payload({ x: null }), { width: 100, height: 80 })).toBeNull()
    expect(normalizePointerContextPayload(payload({ y: Number.NaN }), { width: 100, height: 80 })).toBeNull()
    expect(
      normalizePointerContextPayload(payload({ x: Number.POSITIVE_INFINITY }), { width: 100, height: 80 }),
    ).toBeNull()
  })

  it('clamps pointer coordinates, ttl, and sample rate to lightweight bounds', () => {
    const normalized = normalizePointerContextPayload(payload({ x: -25, y: 999, ttl_ms: 10, sample_rate_hz: 240 }), {
      width: 100,
      height: 80,
    })

    expect(normalized).toEqual(
      expect.objectContaining({
        x: 0,
        y: 80,
        ttlMs: 250,
        sampleRateHz: 30,
      }),
    )

    expect(normalizePointerContextPayload(payload({ ttl_ms: 3000 }), { width: 100, height: 80 })?.ttlMs).toBe(1500)
  })

  it('suppresses click pulse when runtime reduced-motion preference is active', () => {
    const normalized = normalizePointerContextPayload(
      payload({ click_pulse: true, reduced_motion: false }),
      { width: 100, height: 80 },
      true,
    )

    expect(normalized).toEqual(
      expect.objectContaining({
        clickPulse: false,
        reducedMotion: true,
      }),
    )
  })

  it('renders a halo at the normalized coordinate without layout participation', () => {
    render(<PointerContextHighlight pointerContext={payload()} />)

    const root = screen.getByTestId('pointer-context-highlight')
    const halo = screen.getByTestId('pointer-context-halo')
    expect(root).toHaveClass('pointer-events-none')
    expect(halo).toHaveStyle({ left: '48px', top: '72px' })
  })

  it('suppresses click ripple animation when reduced motion is requested', () => {
    render(<PointerContextHighlight pointerContext={payload({ click_pulse: true, reduced_motion: true })} />)

    expect(screen.getByTestId('pointer-context-halo')).toHaveAttribute('data-reduced-motion', 'true')
    expect(screen.queryByTestId('pointer-context-click-ripple')).not.toBeInTheDocument()
  })

  it('expires click ripple before the pointer halo ttl', () => {
    render(<PointerContextHighlight pointerContext={payload({ click_pulse: true, ttl_ms: 1200 })} />)

    expect(screen.getByTestId('pointer-context-click-ripple')).toBeInTheDocument()

    act(() => {
      vi.advanceTimersByTime(700)
    })
    expect(screen.queryByTestId('pointer-context-click-ripple')).not.toBeInTheDocument()
    expect(screen.getByTestId('pointer-context-highlight')).toBeInTheDocument()

    act(() => {
      vi.advanceTimersByTime(500)
    })
    expect(screen.queryByTestId('pointer-context-highlight')).not.toBeInTheDocument()
  })
})
