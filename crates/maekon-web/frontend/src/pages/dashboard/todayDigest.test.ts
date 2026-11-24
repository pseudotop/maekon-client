import { describe, expect, it } from 'vitest'
import type { DailySummary } from '../../api/contracts'
import { isDigestAnalyzed, type OverviewDigest, resolveTodayHeadline, resolveTodayNumbers } from './todayDigest'

const APP = { name: 'VS Code', duration_secs: 60, event_count: 1, frame_count: 1 }

const EMPTY_SUMMARY: DailySummary = {
  date: '2026-11-10',
  total_active_secs: 0,
  total_idle_secs: 0,
  top_apps: [],
  cpu_avg: 0,
  memory_avg_percent: 0,
  frames_captured: 0,
  events_logged: 0,
}

const ACTIVE_SUMMARY: DailySummary = {
  ...EMPTY_SUMMARY,
  total_active_secs: 22_020,
  total_idle_secs: 1_800,
  top_apps: [
    { name: 'VS Code', duration_secs: 9_000, event_count: 40, frame_count: 12 },
    { name: 'Chrome', duration_secs: 4_000, event_count: 20, frame_count: 8 },
  ],
  frames_captured: 20,
  events_logged: 60,
}

function digest(overrides: Partial<OverviewDigest> = {}): OverviewDigest {
  return {
    timeline: [{ segment_id: 'seg-1' }],
    statistics: { deep_work_hours: 2, context_switches: 14, longest_focus_mins: 48 },
    ...overrides,
  }
}

describe('resolveTodayHeadline', () => {
  it('returns no headline when nothing was captured, even if the digest carries numbers', () => {
    // #13735 mutation guard: a response without activity must never be drawn
    // as if it had a summary.
    expect(resolveTodayHeadline(EMPTY_SUMMARY, digest())).toEqual({ kind: 'none' })
    expect(resolveTodayHeadline(undefined, digest())).toEqual({ kind: 'none' })
    expect(resolveTodayHeadline(null, null)).toEqual({ kind: 'none' })
  })

  it('names the top app and the longest focus stretch once the digest has analyzed segments', () => {
    expect(resolveTodayHeadline(ACTIVE_SUMMARY, digest())).toEqual({
      kind: 'heuristic',
      variant: 'focus',
      topApp: 'VS Code',
      longestFocusSecs: 48 * 60,
    })
  })

  it('falls back to the top app alone before analysis, ignoring stale statistics', () => {
    const unanalyzed = digest({ timeline: [] })
    expect(isDigestAnalyzed(unanalyzed)).toBe(false)
    expect(resolveTodayHeadline(ACTIVE_SUMMARY, unanalyzed)).toEqual({
      kind: 'heuristic',
      variant: 'app',
      topApp: 'VS Code',
      longestFocusSecs: 0,
    })
    expect(resolveTodayHeadline(ACTIVE_SUMMARY, undefined)).toMatchObject({ kind: 'heuristic', variant: 'app' })
  })

  it('uses the app headline when the day had no deep-work stretch', () => {
    const noFocus = digest({ statistics: { deep_work_hours: 0, context_switches: 3, longest_focus_mins: 0 } })
    expect(resolveTodayHeadline(ACTIVE_SUMMARY, noFocus)).toMatchObject({ variant: 'app', topApp: 'VS Code' })
  })

  it('says activity is being recorded when no app is named yet', () => {
    const framesOnly: DailySummary = { ...EMPTY_SUMMARY, frames_captured: 3, top_apps: [{ ...APP, name: '  ' }] }
    expect(resolveTodayHeadline(framesOnly, undefined)).toEqual({
      kind: 'heuristic',
      variant: 'activity',
      topApp: null,
      longestFocusSecs: 0,
    })
  })

  it('tolerates a summary without a top-app list (standalone mocks omit it)', () => {
    const withoutApps = { ...ACTIVE_SUMMARY, top_apps: undefined } as unknown as DailySummary
    expect(resolveTodayHeadline(withoutApps, undefined)).toMatchObject({ kind: 'heuristic', variant: 'activity' })
  })

  it('prefers a persisted AI narrative and keeps its provider class', () => {
    const withNarrative = digest({
      ai_narrative: {
        text: '  Mostly refactoring the sync layer.  ',
        provider_class: 'loopback',
        generated_at: '2026-11-10T09:00:00Z',
      },
    })
    expect(resolveTodayHeadline(ACTIVE_SUMMARY, withNarrative)).toEqual({
      kind: 'ai',
      text: 'Mostly refactoring the sync layer.',
      providerClass: 'loopback',
      generatedAt: '2026-11-10T09:00:00Z',
    })
  })

  it('does not claim AI for an empty or whitespace narrative artifact', () => {
    for (const text of [undefined, '', '   ']) {
      const headline = resolveTodayHeadline(
        ACTIVE_SUMMARY,
        digest({ ai_narrative: { text, failure_reason: 'not_generated' } }),
      )
      expect(headline.kind).toBe('heuristic')
    }
  })

  it('never promotes a legacy insight narrative to an AI headline (#11738)', () => {
    const legacy = { ...digest(), insight: { narrative: 'Legacy narrative text', highlights: [] } } as OverviewDigest
    const headline = resolveTodayHeadline(ACTIVE_SUMMARY, legacy)
    expect(headline.kind).toBe('heuristic')
    expect(JSON.stringify(headline)).not.toContain('Legacy narrative text')
  })
})

describe('resolveTodayNumbers', () => {
  it('reads active and idle time from the summary and leaves digest numbers empty before analysis', () => {
    expect(resolveTodayNumbers(ACTIVE_SUMMARY, digest({ timeline: [] }))).toEqual({
      activeSecs: 22_020,
      idleSecs: 1_800,
      deepWorkSecs: null,
      contextSwitches: null,
    })
  })

  it('fills deep work and context switches from an analyzed digest', () => {
    expect(resolveTodayNumbers(ACTIVE_SUMMARY, digest())).toEqual({
      activeSecs: 22_020,
      idleSecs: 1_800,
      deepWorkSecs: 7_200,
      contextSwitches: 14,
    })
  })

  it('reports zero activity, not missing data, for an empty summary', () => {
    expect(resolveTodayNumbers(undefined, undefined)).toEqual({
      activeSecs: 0,
      idleSecs: 0,
      deepWorkSecs: null,
      contextSwitches: null,
    })
  })
})
