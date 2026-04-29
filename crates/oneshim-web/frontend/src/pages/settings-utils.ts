import type {
  AiProviderProfileConfig,
  AiProviderSettings,
  ExternalApiSettings,
  ProviderDiscoveredModel,
  ProviderSurfaceSpec,
} from '../api/client'
import { surfaceOcrRequiresStructuredOutputModel } from '../features/providerSurfaces'

export function backendAllowsSecretEditing(backendKind: string): boolean {
  return backendKind !== 'env' && backendKind !== 'bridge_managed' && backendKind !== 'unavailable'
}

export function supportsProjectionFor(authMode: string, backendKind: string): boolean {
  return authMode === 'api_key' && (backendKind === 'os_secret_store' || backendKind === 'file_secret_store')
}

export function modelDiscoverySensitiveField(field: keyof ExternalApiSettings): boolean {
  return (
    field === 'endpoint' ||
    field === 'api_key_masked' ||
    field === 'provider_type' ||
    field === 'surface_id' ||
    field === 'auth_mode' ||
    field === 'backend_kind' ||
    field === 'has_secret'
  )
}

export function cloneExternalApiSettings(endpoint: ExternalApiSettings | null | undefined): ExternalApiSettings | null {
  return endpoint ? { ...endpoint } : null
}

export function cloneAiProviderProfileConfig(
  aiProvider: AiProviderProfileConfig | AiProviderSettings,
): AiProviderProfileConfig {
  return {
    access_mode: aiProvider.access_mode,
    ocr_provider: aiProvider.ocr_provider,
    llm_provider: aiProvider.llm_provider,
    external_data_policy: aiProvider.external_data_policy,
    allow_unredacted_external_ocr: aiProvider.allow_unredacted_external_ocr,
    ocr_validation: { ...aiProvider.ocr_validation },
    scene_action_override: { ...aiProvider.scene_action_override },
    scene_intelligence: { ...aiProvider.scene_intelligence },
    fallback_to_local: aiProvider.fallback_to_local,
    ocr_api: cloneExternalApiSettings(aiProvider.ocr_api),
    llm_api: cloneExternalApiSettings(aiProvider.llm_api),
  }
}

export function normalizeSavedProfileName(value: string): string {
  return value.trim().replace(/\s+/g, ' ')
}

export function slugifySavedProfileId(value: string): string {
  const normalized = normalizeSavedProfileName(value)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return normalized || 'ai-profile'
}

export function isOcrModelExplicitlyUnsupported(
  detail: ProviderDiscoveredModel | undefined,
  ocrSurface?: ProviderSurfaceSpec,
): boolean {
  return Boolean(
    detail &&
      (detail.supports_ocr === false ||
        detail.ocr_support === 'unsupported' ||
        detail.image_input_support === 'unsupported' ||
        (surfaceOcrRequiresStructuredOutputModel(ocrSurface) && detail.structured_output_support === 'unsupported')),
  )
}

export function isLlmModelExplicitlyUnsupported(detail: ProviderDiscoveredModel | undefined): boolean {
  return Boolean(detail && detail.llm_support === 'unsupported')
}

export function isOcrModelCompatibilityUnknown(
  detail: ProviderDiscoveredModel | undefined,
  ocrSurface?: ProviderSurfaceSpec,
): boolean {
  return Boolean(
    detail &&
      (detail.ocr_support === 'unknown' ||
        detail.image_input_support === 'unknown' ||
        (surfaceOcrRequiresStructuredOutputModel(ocrSurface) && detail.structured_output_support === 'unknown') ||
        detail.supports_ocr == null),
  )
}

export function isLlmModelCompatibilityUnknown(detail: ProviderDiscoveredModel | undefined): boolean {
  return Boolean(detail && detail.llm_support === 'unknown')
}

export function normalizeModelId(value: string | null | undefined): string {
  return (value ?? '').trim().toLowerCase()
}

export function modelDiscoverySignature(endpoint: ExternalApiSettings | null | undefined): string {
  return JSON.stringify({
    provider_type: endpoint?.provider_type ?? '',
    surface_id: endpoint?.surface_id ?? '',
    endpoint: endpoint?.endpoint?.trim() ?? '',
    auth_mode: endpoint?.auth_mode ?? '',
    backend_kind: endpoint?.backend_kind ?? '',
    api_key_masked: endpoint?.api_key_masked ?? '',
    has_secret: Boolean(endpoint?.has_secret),
  })
}
