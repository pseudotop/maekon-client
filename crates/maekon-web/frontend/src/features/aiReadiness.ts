import type {
  AiCapabilityId,
  AiCapabilityReadiness,
  AiReadinessAction,
  AiReadinessDimensions,
  FeatureCapabilitySnapshot,
} from '../api/contracts'

export const AI_CAPABILITY_IDS: readonly AiCapabilityId[] = [
  'chat.subprocess',
  'chat.http_api',
  'chat.local_llm',
  'ocr.capture',
  'ocr.suggestion_analysis',
  'segment_summary',
  'daily_narrative',
]

const CAPABILITY_COPY_KEYS: Record<AiCapabilityId, string> = {
  'chat.subprocess': 'aiReadiness.capability.chatSubprocess',
  'chat.http_api': 'aiReadiness.capability.chatHttpApi',
  'chat.local_llm': 'aiReadiness.capability.chatLocalLlm',
  'ocr.capture': 'aiReadiness.capability.ocrCapture',
  'ocr.suggestion_analysis': 'aiReadiness.capability.ocrSuggestionAnalysis',
  segment_summary: 'aiReadiness.capability.segmentSummary',
  daily_narrative: 'aiReadiness.capability.dailyNarrative',
}

const MISSING_DIMENSIONS: AiReadinessDimensions = {
  compiled_capability: false,
  selected_access_mode: 'provider_api_key',
  access_mode_compatible: false,
  endpoint_or_profile_configured: false,
  provider_detection: 'not_detected',
  provider_auth: 'unverified',
  provider_invocation: 'unverified',
  model_availability: 'unverified',
  runtime_flag_enabled: false,
  consent: [],
  apply_requirement: 'restart',
  apply_pending: false,
  privacy_gate: 'enforced_at_invocation',
  egress_gate: 'enforced_at_invocation',
  budget_gate: 'enforced_at_invocation',
  audit_gate: 'enforced_at_invocation',
}

/**
 * Single frontend seam for Settings, Chat, suggestions, and summary surfaces.
 * Missing/old snapshots fail closed and cannot inherit a generic app/server
 * `Connected` badge as evidence of LLM readiness (#11735).
 */
export function aiCapabilityReadiness(
  snapshot: FeatureCapabilitySnapshot | null | undefined,
  capabilityId: AiCapabilityId,
): AiCapabilityReadiness {
  const readiness = snapshot?.ai_readiness?.capabilities.find((candidate) => candidate.capability_id === capabilityId)
  if (readiness) return readiness

  return {
    capability_id: capabilityId,
    status: 'blocked',
    reason_code: 'compiled_capability_missing',
    action: 'none',
    action_copy_key: 'aiReadiness.action.none',
    dimensions: MISSING_DIMENSIONS,
  }
}

export function aiCapabilityReady(
  snapshot: FeatureCapabilitySnapshot | null | undefined,
  capabilityId: AiCapabilityId,
): boolean {
  return aiCapabilityReadiness(snapshot, capabilityId).status === 'ready'
}

/** The backend owns the reason→action mapping; consumers only translate it. */
export function aiReadinessActionCopyKey(readiness: AiCapabilityReadiness): string {
  return readiness.action_copy_key
}

export function aiReadinessReasonCopyKey(readiness: AiCapabilityReadiness): string {
  return `aiReadiness.reason.${readiness.reason_code}`
}

export function aiCapabilityCopyKey(capabilityId: AiCapabilityId): string {
  return CAPABILITY_COPY_KEYS[capabilityId]
}

const ACTION_ROUTES: Partial<Record<AiReadinessAction, string>> = {
  open_ai_settings: '/settings/ai-automation',
  open_privacy_consent: '/privacy/consent',
  enable_feature: '/settings/ai-automation',
  install_provider: '/settings/ai-automation',
  authenticate_provider: '/settings/ai-automation',
  verify_provider_invocation: '/settings/ai-automation',
  select_model: '/settings/ai-automation',
  apply_hot_rewire: '/settings/ai-automation',
  review_privacy: '/privacy/consent',
  review_egress: '/privacy/egress',
  review_budget: '/settings/ai-automation',
  review_audit: '/audit/summary',
}

/** Stable in-app setup route shared by every readiness consumer. */
export function aiReadinessActionRoute(readiness: AiCapabilityReadiness): string | null {
  return ACTION_ROUTES[readiness.action] ?? null
}

export function chatCapabilityForTransport(transport: 'subprocess' | 'http_api' | 'local_llm'): AiCapabilityId {
  if (transport === 'subprocess') return 'chat.subprocess'
  if (transport === 'local_llm') return 'chat.local_llm'
  return 'chat.http_api'
}

export type ChatTransport = 'subprocess' | 'http_api' | 'local_llm'

export const CHAT_CAPABILITY_IDS: readonly AiCapabilityId[] = ['chat.subprocess', 'chat.http_api', 'chat.local_llm']

const CHAT_TRANSPORT_PRIORITY: readonly ChatTransport[] = ['subprocess', 'http_api', 'local_llm']

/**
 * Pick only a transport whose complete backend-owned readiness contract is
 * ready. The order is stable so two renders of the same snapshot cannot send
 * the user down different provider paths.
 */
export function bestReadyChatTransport(
  snapshot: FeatureCapabilitySnapshot | null | undefined,
  hasHttpSurface: boolean,
): ChatTransport | null {
  return CHAT_TRANSPORT_PRIORITY.find((transport) => canCreateChatSession(snapshot, transport, hasHttpSurface)) ?? null
}

/**
 * Recommend an invocation-ready provider when configuration is the only
 * remaining blocker. This keeps an installed CLI discoverable without
 * weakening the create gate: the selected readiness notice still owns the
 * exact blocker and setup action.
 */
export function recommendedChatTransport(
  snapshot: FeatureCapabilitySnapshot | null | undefined,
  hasHttpSurface: boolean,
): ChatTransport | null {
  const ready = bestReadyChatTransport(snapshot, hasHttpSurface)
  if (ready) return ready

  return (
    CHAT_TRANSPORT_PRIORITY.find((transport) => {
      if (transport === 'http_api' && !hasHttpSurface) return false
      const dimensions = aiCapabilityReadiness(snapshot, chatCapabilityForTransport(transport)).dimensions
      return (
        dimensions.compiled_capability &&
        dimensions.provider_detection === 'detected' &&
        dimensions.provider_invocation === 'ready' &&
        (transport !== 'local_llm' || dimensions.model_availability === 'available')
      )
    }) ?? null
  )
}

/** Safe provider identity for the pre-session Chat surface. */
export function chatProviderIdentity(
  snapshot: FeatureCapabilitySnapshot | null | undefined,
  transport: ChatTransport | null,
  httpProviderName?: string | null,
): string | null {
  if (!transport) return null
  if (transport === 'http_api') return httpProviderName?.trim() || null
  if (transport === 'local_llm') return 'Ollama'

  const candidates = (snapshot?.features ?? [])
    .filter(
      (feature) =>
        feature.feature_id.endsWith('.subprocess_cli') && feature.provider_cli_readiness === 'invocation_ready',
    )
    .sort(
      (left, right) =>
        Number(right.preferred) - Number(left.preferred) || left.feature_id.localeCompare(right.feature_id),
    )
  const selected = candidates[0]
  if (!selected) return null
  return selected.provider_cli_discovery?.candidate_name?.trim() || selected.feature_id.split('.')[1] || null
}

/** Fail-closed creation gate shared by every Chat create affordance. */
export function canCreateChatSession(
  snapshot: FeatureCapabilitySnapshot | null | undefined,
  transport: 'subprocess' | 'http_api' | 'local_llm',
  hasHttpSurface: boolean,
): boolean {
  if (transport === 'http_api' && !hasHttpSurface) return false
  return aiCapabilityReady(snapshot, chatCapabilityForTransport(transport))
}
