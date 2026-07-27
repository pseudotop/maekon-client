import { describe, expect, it } from 'vitest'
import type { AiProviderProfileConfig, AppSettings } from './contracts'
import {
  normalizeAiAccessModeForUi,
  normalizeAiProviderProfileForUi,
  normalizeAppSettingsForUi,
} from './settings-normalization'

function profile(accessMode: string): AiProviderProfileConfig {
  return {
    access_mode: accessMode,
    ocr_provider: 'Local',
    llm_provider: 'Remote',
    external_data_policy: 'PiiFilterStandard',
    bypass_pii_filter_for_external_ocr: false,
    fallback_to_local: true,
    ocr_validation: {
      enabled: true,
      min_confidence: 0.6,
      max_invalid_ratio: 0.4,
    },
    scene_action_override: {
      enabled: false,
      reason: '',
      approved_by: '',
      expires_at: null,
    },
    scene_intelligence: {
      enabled: true,
      show_overlay: true,
      allow_action_execution: false,
      min_scene_confidence: 0.6,
      max_elements: 100,
      calibration_enabled: true,
      calibration_min_elements: 3,
      calibration_min_average_confidence: 0.6,
    },
    ocr_api: null,
    llm_api: null,
  }
}

describe('settings wire normalization', () => {
  it.each([
    ['provider_api_key', 'ProviderApiKey'],
    ['local_model', 'LocalModel'],
    ['provider_subscription_cli', 'ProviderSubscriptionCli'],
    ['provider_o_auth', 'ProviderOAuth'],
    ['ProviderSubscriptionCli', 'ProviderSubscriptionCli'],
  ])('normalizes access mode %s to %s', (wireValue, expected) => {
    expect(normalizeAiAccessModeForUi(wireValue)).toBe(expected)
  })

  it('preserves an unknown future access mode for explicit UI handling', () => {
    expect(normalizeAiAccessModeForUi('provider_future_transport')).toBe('provider_future_transport')
  })

  it('normalizes both the active provider and saved profiles without mutation', () => {
    const active = {
      ...profile('provider_subscription_cli'),
      active_profile_id: 'cli',
      saved_profiles: [
        {
          profile_id: 'cli',
          name: 'CLI',
          ai_provider: profile('provider_subscription_cli'),
          updated_at: null,
        },
      ],
    }
    const settings = { ai_provider: active } as AppSettings

    const normalized = normalizeAppSettingsForUi(settings)

    expect(normalized.ai_provider.access_mode).toBe('ProviderSubscriptionCli')
    expect(normalized.ai_provider.saved_profiles?.[0]?.ai_provider.access_mode).toBe('ProviderSubscriptionCli')
    expect(settings.ai_provider.access_mode).toBe('provider_subscription_cli')
  })

  it('normalizes a standalone profile object', () => {
    expect(normalizeAiProviderProfileForUi(profile('provider_o_auth')).access_mode).toBe('ProviderOAuth')
  })
})
