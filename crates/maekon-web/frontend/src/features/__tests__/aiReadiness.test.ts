import { describe, expect, it } from 'vitest'
import type { AiCapabilityReadiness, FeatureCapabilitySnapshot } from '../../api/contracts'
import {
  AI_CAPABILITY_IDS,
  aiCapabilityReadiness,
  aiCapabilityReady,
  aiReadinessActionCopyKey,
  aiReadinessActionRoute,
  bestReadyChatTransport,
  canCreateChatSession,
  chatCapabilityForTransport,
  chatProviderIdentity,
  recommendedChatTransport,
} from '../aiReadiness'

function readiness(overrides: Partial<AiCapabilityReadiness> = {}): AiCapabilityReadiness {
  return {
    capability_id: 'chat.subprocess',
    status: 'ready',
    reason_code: 'ready',
    action: 'none',
    action_copy_key: 'aiReadiness.action.none',
    dimensions: {
      compiled_capability: true,
      selected_access_mode: 'provider_subscription_cli',
      access_mode_compatible: true,
      endpoint_or_profile_configured: true,
      provider_detection: 'detected',
      provider_auth: 'ready',
      provider_invocation: 'ready',
      model_availability: 'not_required',
      runtime_flag_enabled: true,
      consent: [],
      apply_requirement: 'restart',
      apply_pending: false,
      privacy_gate: 'enforced_at_invocation',
      egress_gate: 'enforced_at_invocation',
      budget_gate: 'enforced_at_invocation',
      audit_gate: 'enforced_at_invocation',
    },
    ...overrides,
  }
}

function snapshot(item: AiCapabilityReadiness): FeatureCapabilitySnapshot {
  return {
    features: [],
    ai_readiness: { contract_version: 1, capabilities: [item] },
  }
}

describe('shared AI readiness consumer seam (#11735)', () => {
  it('returns the authoritative backend readiness without recomputing it', () => {
    const item = readiness({
      status: 'blocked',
      reason_code: 'access_mode_mismatch',
      action: 'open_ai_settings',
      action_copy_key: 'aiReadiness.action.openAiSettings',
    })

    expect(aiCapabilityReadiness(snapshot(item), 'chat.subprocess')).toBe(item)
    expect(aiReadinessActionCopyKey(item)).toBe('aiReadiness.action.openAiSettings')
  })

  it('requires invocation-ready, not merely auth-ready', () => {
    const item = readiness({
      status: 'blocked',
      reason_code: 'provider_invocation_unverified',
      action: 'verify_provider_invocation',
      action_copy_key: 'aiReadiness.action.verifyProviderInvocation',
      dimensions: {
        ...readiness().dimensions,
        provider_auth: 'ready',
        provider_invocation: 'unverified',
      },
    })

    expect(aiCapabilityReady(snapshot(item), 'chat.subprocess')).toBe(false)
  })

  it('fails closed when an old snapshot has no readiness contract', () => {
    const result = aiCapabilityReadiness({ features: [] }, 'daily_narrative')

    expect(result.status).toBe('blocked')
    expect(result.reason_code).toBe('compiled_capability_missing')
    expect(result.action).toBe('none')
    expect(aiReadinessActionRoute(result)).toBeNull()
  })

  it('does not accept a generic connected flag as LLM readiness', () => {
    const legacy = { features: [], connected: true } as FeatureCapabilitySnapshot & {
      connected: boolean
    }

    expect(aiCapabilityReady(legacy, 'chat.http_api')).toBe(false)
  })

  it('keeps every stable capability represented and routes setup actions consistently', () => {
    expect(AI_CAPABILITY_IDS).toHaveLength(7)
    const item = readiness({
      action: 'open_privacy_consent',
      action_copy_key: 'aiReadiness.action.openPrivacyConsent',
    })
    expect(aiReadinessActionRoute(item)).toBe('/privacy/consent')

    const egressItem = readiness({ action: 'review_egress' })
    expect(aiReadinessActionRoute(egressItem)).toBe('/privacy/egress')
  })

  it('maps every Chat transport to its own authoritative readiness capability', () => {
    expect(chatCapabilityForTransport('local_llm')).toBe('chat.local_llm')
    expect(chatCapabilityForTransport('subprocess')).toBe('chat.subprocess')
    expect(chatCapabilityForTransport('http_api')).toBe('chat.http_api')
  })

  it('fails closed when creating a Chat session without invocation readiness', () => {
    expect(canCreateChatSession(undefined, 'subprocess', false)).toBe(false)
    expect(canCreateChatSession(snapshot(readiness()), 'subprocess', false)).toBe(true)
    expect(canCreateChatSession(snapshot(readiness({ capability_id: 'chat.http_api' })), 'http_api', false)).toBe(false)
    expect(canCreateChatSession(snapshot(readiness({ capability_id: 'chat.http_api' })), 'http_api', true)).toBe(true)
    expect(
      canCreateChatSession(
        snapshot(readiness({ capability_id: 'chat.local_llm', status: 'blocked' })),
        'local_llm',
        false,
      ),
    ).toBe(false)
  })

  it('selects the only fully ready transport deterministically', () => {
    const local = readiness({
      capability_id: 'chat.local_llm',
      dimensions: { ...readiness().dimensions, model_availability: 'available' },
    })
    const blockedCli = readiness({
      status: 'blocked',
      reason_code: 'provider_invocation_unavailable',
      dimensions: { ...readiness().dimensions, provider_invocation: 'unavailable' },
    })
    const value: FeatureCapabilitySnapshot = {
      features: [],
      ai_readiness: { contract_version: 1, capabilities: [blockedCli, local] },
    }

    expect(bestReadyChatTransport(value, false)).toBe('local_llm')
  })

  it('recommends an invocation-ready CLI without bypassing its setup blocker', () => {
    const cli = readiness({
      status: 'blocked',
      reason_code: 'access_mode_mismatch',
      action: 'open_ai_settings',
      dimensions: { ...readiness().dimensions, access_mode_compatible: false },
    })
    const value: FeatureCapabilitySnapshot = {
      features: [
        {
          feature_id: 'provider_surface.openai.subprocess_cli',
          maturity: 'stable',
          availability: 'available',
          provider_cli_readiness: 'invocation_ready',
          provider_cli_discovery: {
            candidate_name: 'Codex CLI',
            executable_hint: 'codex',
            version_status: 'not_checked',
            dependency_status: 'ready',
            status_reason: null,
            env_refresh_required: false,
          },
          preferred: true,
          requires: [],
          status_reason: null,
          status_copy_key: null,
          setup_copy_key: null,
          setup_docs_url: null,
          configuration_env_vars: [],
        },
      ],
      ai_readiness: { contract_version: 1, capabilities: [cli] },
    }

    expect(recommendedChatTransport(value, false)).toBe('subprocess')
    expect(canCreateChatSession(value, 'subprocess', false)).toBe(false)
    expect(chatProviderIdentity(value, 'subprocess')).toBe('Codex CLI')
  })

  it('returns an explicit no-provider state when every provider is unavailable', () => {
    expect(bestReadyChatTransport({ features: [] }, false)).toBeNull()
    expect(recommendedChatTransport({ features: [] }, false)).toBeNull()
    expect(chatProviderIdentity({ features: [] }, null)).toBeNull()
  })
})
