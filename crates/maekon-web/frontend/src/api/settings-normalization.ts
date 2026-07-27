import type { AiProviderProfileConfig, AppSettings } from './contracts'

const UI_ACCESS_MODE_BY_NORMALIZED_TOKEN: Readonly<Record<string, string>> = {
  providerapikey: 'ProviderApiKey',
  localmodel: 'LocalModel',
  providersubscriptioncli: 'ProviderSubscriptionCli',
  provideroauth: 'ProviderOAuth',
}

/**
 * Convert Rust's snake_case wire representation into the canonical values used
 * by the settings form. The backend also accepts the legacy PascalCase values,
 * so normalizing both shapes keeps GET and POST responses symmetric.
 */
export function normalizeAiAccessModeForUi(value: string | null | undefined): string {
  const trimmed = value?.trim() ?? ''
  if (!trimmed) return trimmed

  const normalizedToken = trimmed.replace(/[^a-z0-9]/gi, '').toLowerCase()
  return UI_ACCESS_MODE_BY_NORMALIZED_TOKEN[normalizedToken] ?? trimmed
}

export function normalizeAiProviderProfileForUi<T extends AiProviderProfileConfig>(profile: T): T {
  return {
    ...profile,
    access_mode: normalizeAiAccessModeForUi(profile.access_mode),
  }
}

/** Normalize current and saved-profile wire enums without mutating API data. */
export function normalizeAppSettingsForUi(settings: AppSettings): AppSettings {
  // Defensive: tolerate partial settings payloads without an ai_provider block
  // (contract test client-settings-update encodes this tolerance; #8685).
  if (!settings.ai_provider) return settings
  const savedProfiles = settings.ai_provider.saved_profiles?.map((profile) => ({
    ...profile,
    ai_provider: normalizeAiProviderProfileForUi(profile.ai_provider),
  }))

  return {
    ...settings,
    ai_provider: {
      ...normalizeAiProviderProfileForUi(settings.ai_provider),
      saved_profiles: savedProfiles,
    },
  }
}
