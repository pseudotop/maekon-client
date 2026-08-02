/**
 * #9611 WD-02.3 — section-state derivations.
 *
 * These are the decisions that keep "the server had nothing" apart from "the
 * server could not answer". Testing them here, as pure functions, is what makes
 * the distinction checkable at all — in a rendered tree both look like a panel
 * with no rows.
 */

import { describe, expect, it } from 'vitest'
// The client-subtree mirror (#9625): `clients/maekon-client` is exported
// verbatim to the public repo, so a path that climbed out of it would
// resolve here and fail there.
import fixture from '../../../../../../../api/fixtures/context-home.v1.json'
import type { ContextHomeSnapshot, ContextHomeThreadSection } from '../../../api/contextHome'
import {
  formatAsOf,
  hiddenRowCount,
  homeCompleteness,
  isUnserved,
  MAX_RENDERED_PARTICIPANTS,
  MAX_RENDERED_PROJECTS,
  MAX_RENDERED_THREADS,
  sectionRenderState,
} from '../homeSections'

const snapshot = fixture as unknown as ContextHomeSnapshot

function threadSection(over: Partial<ContextHomeThreadSection> = {}): ContextHomeThreadSection {
  return { status: 'ready', items: [], truncated: false, next_cursor: null, unavailable_reason: null, ...over }
}

describe('sectionRenderState', () => {
  it('separates an unserved section from an answered-but-empty one', () => {
    // Both carry zero rows. Collapsing them is the defect the server contract
    // was shaped to prevent, so length must never be the deciding signal.
    expect(sectionRenderState(threadSection({ status: 'unavailable' }))).toBe('unavailable')
    expect(sectionRenderState(threadSection({ status: 'ready' }))).toBe('empty')
    expect(sectionRenderState(threadSection({ status: 'empty' }))).toBe('empty')
  })

  it('reports ready only when the server said ready AND there are rows', () => {
    const withRows = threadSection({ items: [{ thread_id: 't' }] as never })
    expect(sectionRenderState(withRows)).toBe('ready')
  })

  it('treats an unrecognised status as unknown rather than as answered', () => {
    // A server that grows a new status must not have its new state silently
    // rendered as "you have nothing".
    expect(sectionRenderState(threadSection({ status: 'degraded' }))).toBe('unknown')
    expect(sectionRenderState(undefined)).toBe('unknown')
  })

  it('counts unknown as unserved', () => {
    expect(isUnserved('unknown')).toBe(true)
    expect(isUnserved('unavailable')).toBe(true)
    expect(isUnserved('empty')).toBe(false)
    expect(isUnserved('ready')).toBe(false)
  })
})

describe('homeCompleteness', () => {
  const base = (mail: string, messenger: string, projects: string): ContextHomeSnapshot =>
    ({
      ...snapshot,
      mail: threadSection({ status: mail, items: mail === 'ready' ? ([{ thread_id: 'a' }] as never) : [] }),
      messenger: threadSection({
        status: messenger,
        items: messenger === 'ready' ? ([{ thread_id: 'b' }] as never) : [],
      }),
      projects: threadSection({
        status: projects,
        items: projects === 'ready' ? ([{ project_id: 'p' }] as never) : [],
      }) as never,
    }) as ContextHomeSnapshot

  it('calls it partial when some sections answered and at least one did not', () => {
    // This is the state the page most needs to keep visible: rendering it as
    // plain success quietly under-reports the operator's actual context.
    expect(homeCompleteness(base('ready', 'ready', 'unavailable'))).toBe('partial')
    expect(homeCompleteness(base('ready', 'unavailable', 'unavailable'))).toBe('partial')
    expect(homeCompleteness(base('empty', 'empty', 'unavailable'))).toBe('partial')
  })

  it('calls it unavailable only when nothing could be served', () => {
    expect(homeCompleteness(base('unavailable', 'unavailable', 'unavailable'))).toBe('unavailable')
  })

  it('separates a fully empty context from an unavailable one', () => {
    expect(homeCompleteness(base('empty', 'empty', 'empty'))).toBe('empty')
  })

  it('calls it complete when every section answered with rows', () => {
    expect(homeCompleteness(base('ready', 'ready', 'ready'))).toBe('complete')
  })

  it('reads the committed fixture as partial', () => {
    // The fixture deliberately carries a served mail/messenger pair and an
    // unavailable projects section — if it ever became all-happy-path, the
    // partial branch would stop being exercised by anything real.
    expect(homeCompleteness(snapshot)).toBe('partial')
  })
})

describe('hiddenRowCount', () => {
  it('reports what the UI is holding back, never a negative', () => {
    const section = threadSection({ items: Array.from({ length: 11 }, (_, i) => ({ thread_id: `t${i}` })) as never })
    expect(hiddenRowCount(section, MAX_RENDERED_THREADS)).toBe(11 - MAX_RENDERED_THREADS)
    expect(hiddenRowCount(threadSection({ items: [] }), MAX_RENDERED_THREADS)).toBe(0)
    expect(hiddenRowCount(undefined, MAX_RENDERED_THREADS)).toBe(0)
  })
})

describe('render caps', () => {
  it('stay below the server-side payload bounds so truncation is legible, not protective', () => {
    // The server caps at 20 threads / 10 projects / 12 participants. If a UI cap
    // met or exceeded those, the "+N more" affordance would never appear and the
    // bounded-render requirement would be satisfied only by accident.
    expect(MAX_RENDERED_THREADS).toBeLessThan(20)
    expect(MAX_RENDERED_PROJECTS).toBeLessThan(10)
    expect(MAX_RENDERED_PARTICIPANTS).toBeLessThan(12)
  })
})

describe('formatAsOf', () => {
  it('formats in the zone the server named, not the runtime default', () => {
    // The demo's day boundaries are KST. Formatting in browser-local time moves
    // items across days for anyone outside that zone.
    const seoul = formatAsOf('2026-07-31T20:00:00Z', 'Asia/Seoul', 'en-US')
    const utc = formatAsOf('2026-07-31T20:00:00Z', 'UTC', 'en-US')
    expect(seoul).not.toBe(utc)
    // 20:00Z is the next calendar day in Seoul (+9).
    expect(seoul).toContain('Aug')
    expect(utc).toContain('Jul')
  })

  it('falls back to the raw timestamp rather than throwing inside a render', () => {
    const raw = '2026-07-31T20:00:00Z'
    expect(formatAsOf(raw, 'Not/AZone', 'en-US')).toBe(raw)
  })
})
