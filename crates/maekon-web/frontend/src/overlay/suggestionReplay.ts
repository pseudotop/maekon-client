import type { SuggestionGuiAnchorPayload, SuggestionSurfacePlacement, SuggestionViewDto } from './types'

export type SuggestionReplayPhase = 'marker_opened' | 'proposal_visible' | 'feedback_submitted'
export type SuggestionReplayAction = 'accept' | 'reject' | 'defer' | 'explain'

export interface SuggestionReplayEventPayload {
  eventName: string
  phase: SuggestionReplayPhase
  suggestionId: string | null
  targetId: string | null
  surfacePlacement: SuggestionSurfacePlacement
  appName: string | null
  windowTitle: string | null
  action: SuggestionReplayAction | null
  auditReady: boolean
  rawContextIncluded: false
}

interface BuildSuggestionReplayEventInput {
  phase: SuggestionReplayPhase
  placement: SuggestionSurfacePlacement
  anchor?: SuggestionGuiAnchorPayload | null
  suggestion?: SuggestionViewDto | null
  action?: SuggestionReplayAction | null
}

function safeWindowTitle(anchor?: SuggestionGuiAnchorPayload | null) {
  if (!anchor) return null
  const title = anchor.active_process.window_title.trim()
  if (!title) return null
  if (title === anchor.active_app || title === anchor.active_process.process_name) return title
  return null
}

export function buildSuggestionReplayEvent({
  phase,
  placement,
  anchor,
  suggestion,
  action = null,
}: BuildSuggestionReplayEventInput): SuggestionReplayEventPayload {
  return {
    action,
    appName: anchor?.active_app ?? suggestion?.context_scope?.app_name ?? null,
    auditReady: true,
    eventName: `suggestion.replay.${phase}`,
    phase,
    rawContextIncluded: false,
    suggestionId: suggestion?.id ?? null,
    surfacePlacement: placement,
    targetId: anchor?.target_entity.entity_id ?? suggestion?.context_scope?.target_id ?? null,
    windowTitle: safeWindowTitle(anchor),
  }
}

export async function recordSuggestionReplayEvent(payload: SuggestionReplayEventPayload) {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke('record_suggestion_replay_event', { payload })
  } catch (e) {
    console.warn('record_suggestion_replay_event failed:', e)
  }
}
