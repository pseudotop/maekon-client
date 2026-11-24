/**
 * First-screen digest model (#13735).
 *
 * Arranges data the client already has — the daily summary and the heuristic
 * daily digest — into the overview's one-line summary and key numbers. It does
 * not generate or score anything new.
 *
 * Two rules keep the first screen honest:
 *  - No captured activity means no summary (`kind: 'none'`), never a sentence
 *    built from zeros.
 *  - Only a persisted `ai_narrative` artifact with text yields an AI headline.
 *    A legacy `insight.narrative` is never promoted to an AI claim (#11738:
 *    the artifact is the provenance authority).
 */

import type {
  AiSummaryArtifact,
  AiSummaryProviderClass,
  DailyDigestStatistics,
  DailySummary,
} from '../../api/contracts'
import { hasCapturedActivity } from '../../components/CaptureProcessingStatus'

/**
 * The digest fields the overview reads. Standalone mocks omit the provenance
 * fields, so they stay optional here.
 */
export interface OverviewDigest {
  timeline: readonly unknown[]
  statistics: Pick<DailyDigestStatistics, 'deep_work_hours' | 'context_switches' | 'longest_focus_mins'>
  ai_narrative?: AiSummaryArtifact | null
}

export type HeuristicHeadlineVariant = 'focus' | 'app' | 'activity'

export type TodayHeadline =
  | {
      kind: 'ai'
      text: string
      providerClass: AiSummaryProviderClass
      generatedAt: string | null
    }
  | {
      kind: 'heuristic'
      variant: HeuristicHeadlineVariant
      topApp: string | null
      /** Longest deep-work stretch; `0` until the digest has analyzed segments. */
      longestFocusSecs: number
    }
  | { kind: 'none' }

export interface TodayNumbers {
  activeSecs: number
  idleSecs: number
  /** `null` until the heuristic digest has analyzed at least one segment. */
  deepWorkSecs: number | null
  /** `null` until the heuristic digest has analyzed at least one segment. */
  contextSwitches: number | null
}

/** True once the heuristic digest has at least one analyzed segment. */
export function isDigestAnalyzed(digest: OverviewDigest | null | undefined): digest is OverviewDigest {
  return Boolean(digest && digest.timeline.length > 0)
}

function topAppName(summary: DailySummary): string | null {
  // Standalone and test mocks can omit the list; treat it as empty.
  const app = (summary.top_apps ?? []).find((candidate) => candidate.name?.trim())
  return app ? app.name.trim() : null
}

export function resolveTodayHeadline(
  summary: DailySummary | null | undefined,
  digest: OverviewDigest | null | undefined,
): TodayHeadline {
  const narrative = digest?.ai_narrative
  const aiText = narrative?.text?.trim()
  if (aiText) {
    return {
      kind: 'ai',
      text: aiText,
      providerClass: narrative?.provider_class ?? 'unknown',
      generatedAt: narrative?.generated_at ?? null,
    }
  }

  if (!hasCapturedActivity(summary)) {
    return { kind: 'none' }
  }

  const topApp = topAppName(summary)
  // Deep work is a key number of its own; the headline adds what the numbers
  // do not say (where the time went, the longest uninterrupted stretch).
  const longestFocusSecs = isDigestAnalyzed(digest) ? Math.round(digest.statistics.longest_focus_mins * 60) : 0

  let variant: HeuristicHeadlineVariant = 'activity'
  if (topApp && longestFocusSecs > 0) {
    variant = 'focus'
  } else if (topApp) {
    variant = 'app'
  }

  return { kind: 'heuristic', variant, topApp, longestFocusSecs }
}

export function resolveTodayNumbers(
  summary: DailySummary | null | undefined,
  digest: OverviewDigest | null | undefined,
): TodayNumbers {
  const analyzed = isDigestAnalyzed(digest)
  return {
    activeSecs: summary?.total_active_secs ?? 0,
    idleSecs: summary?.total_idle_secs ?? 0,
    deepWorkSecs: analyzed ? Math.round(digest.statistics.deep_work_hours * 3600) : null,
    contextSwitches: analyzed ? digest.statistics.context_switches : null,
  }
}
