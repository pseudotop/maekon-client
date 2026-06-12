import type { CSSProperties } from 'react'
import { useEffect, useMemo, useState } from 'react'
import type { PointerContextPayload } from '../types'

const MAX_SAMPLE_RATE_HZ = 30
const MIN_TTL_MS = 250
const MAX_TTL_MS = 1500
const CLICK_RIPPLE_MS = 650

interface ViewportBounds {
  width: number
  height: number
}

export interface NormalizedPointerContext {
  x: number
  y: number
  clickPulse: boolean
  reducedMotion: boolean
  ttlMs: number
  sampleRateHz: number
}

interface PointerContextHighlightProps {
  pointerContext: PointerContextPayload | null
}

function clamp(value: number, min: number, max: number) {
  return Math.max(min, Math.min(value, max))
}

function currentViewport(): ViewportBounds {
  return {
    width: Math.max(0, window.innerWidth || 0),
    height: Math.max(0, window.innerHeight || 0),
  }
}

function currentReducedMotionPreference() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
}

export function normalizePointerContextPayload(
  payload: PointerContextPayload | null,
  viewport: ViewportBounds = currentViewport(),
  prefersReducedMotion: boolean = currentReducedMotionPreference(),
): NormalizedPointerContext | null {
  if (!payload?.enabled) return null
  if (payload.x === null || payload.y === null) return null
  if (!Number.isFinite(payload.x) || !Number.isFinite(payload.y)) return null

  const width = Math.max(0, viewport.width)
  const height = Math.max(0, viewport.height)
  const reducedMotion = !!payload.reduced_motion || prefersReducedMotion

  return {
    x: clamp(payload.x, 0, width),
    y: clamp(payload.y, 0, height),
    clickPulse: !!payload.click_pulse && !reducedMotion,
    reducedMotion,
    ttlMs: clamp(payload.ttl_ms || MIN_TTL_MS, MIN_TTL_MS, MAX_TTL_MS),
    sampleRateHz: clamp(payload.sample_rate_hz || MAX_SAMPLE_RATE_HZ, 1, MAX_SAMPLE_RATE_HZ),
  }
}

export function PointerContextHighlight({ pointerContext }: PointerContextHighlightProps) {
  const normalized = useMemo(() => normalizePointerContextPayload(pointerContext), [pointerContext])
  const [visible, setVisible] = useState(false)
  const [rippleVisible, setRippleVisible] = useState(false)

  useEffect(() => {
    if (!normalized) {
      setVisible(false)
      setRippleVisible(false)
      return
    }

    setVisible(true)
    setRippleVisible(normalized.clickPulse)

    const hideTimer = window.setTimeout(() => setVisible(false), normalized.ttlMs)
    const rippleTimer = normalized.clickPulse ? window.setTimeout(() => setRippleVisible(false), CLICK_RIPPLE_MS) : null

    return () => {
      window.clearTimeout(hideTimer)
      if (rippleTimer !== null) window.clearTimeout(rippleTimer)
    }
  }, [normalized])

  if (!normalized || !visible) return null

  const anchorStyle: CSSProperties = {
    left: `${normalized.x}px`,
    top: `${normalized.y}px`,
    transform: 'translate(-50%, -50%)',
  }

  const transitionStyle: CSSProperties = normalized.reducedMotion
    ? { transition: 'none' }
    : { transition: 'opacity 120ms ease-out, transform 120ms ease-out' }

  return (
    <div
      aria-hidden="true"
      className="pointer-events-none fixed inset-0 z-detection"
      data-sample-rate-hz={normalized.sampleRateHz}
      data-testid="pointer-context-highlight"
    >
      <div
        className={`absolute h-11 w-11 rounded-full border-2 border-brand-signal bg-brand-signal/10 ${
          normalized.reducedMotion ? '' : 'animate-pulse'
        }`}
        data-reduced-motion={String(normalized.reducedMotion)}
        data-testid="pointer-context-halo"
        style={{
          ...anchorStyle,
          ...transitionStyle,
          boxShadow: '0 0 0 5px rgb(var(--brand-signal) / 0.16), 0 0 18px rgb(var(--brand-signal) / 0.38)',
        }}
      />
      {rippleVisible && (
        <div
          className="absolute h-14 w-14 rounded-full border-2 border-brand-signal"
          data-testid="pointer-context-click-ripple"
          style={{
            ...anchorStyle,
            animation: 'ping 650ms cubic-bezier(0, 0, 0.2, 1)',
            opacity: 0.75,
          }}
        />
      )}
    </div>
  )
}
