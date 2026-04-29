import type { useTranslation } from 'react-i18next'
import type { ExternalApiSettings, ProviderSurfaceSpec } from '../../api/client'
import type { EndpointSurfaceKind } from '../../features/providerSurfaces'

type TFunction = ReturnType<typeof useTranslation>['t']

export function toDateTimeLocalValue(value: string | null | undefined): string {
  if (!value) return ''
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

export function toRfc3339OrNull(value: string): string | null {
  if (!value.trim()) return null
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return null
  return date.toISOString()
}

export function credentialBackendLabel(t: TFunction, backendKind: string | null | undefined): string {
  switch ((backendKind ?? '').trim()) {
    case 'os_secret_store':
      return t('settingsAutomation.backendOsSecretStore')
    case 'file_secret_store':
      return t('settingsAutomation.backendFileSecretStore')
    case 'env':
      return t('settingsAutomation.backendEnv')
    case 'bridge_managed':
      return t('settingsAutomation.backendBridgeManaged')
    default:
      return t('settingsAutomation.backendUnavailable')
  }
}

export function apiKeyPlaceholder(t: TFunction, settings: ExternalApiSettings | null | undefined): string {
  if (!settings) {
    return t('settingsAutomation.apiKeyPlaceholder')
  }
  if (settings.secret_display_hint) {
    return settings.secret_display_hint
  }
  if (settings.has_secret) {
    return t('settingsAutomation.apiKeyStoredPlaceholder', {
      backend: credentialBackendLabel(t, settings.backend_kind),
    })
  }
  return t('settingsAutomation.apiKeyPlaceholder')
}

export function shouldShowBackendManagedHint(settings: ExternalApiSettings | null | undefined): boolean {
  if (!settings?.has_secret) return false
  if (settings.api_key_masked.trim().length > 0) return false
  return settings.backend_kind !== 'unavailable'
}

export function supportsProjectionToggle(settings: ExternalApiSettings | null | undefined): boolean {
  if (!settings) return false
  return (
    settings.auth_mode === 'api_key' &&
    (settings.backend_kind === 'os_secret_store' || settings.backend_kind === 'file_secret_store')
  )
}

export function executionKindLabel(t: TFunction, executionKind: string | null | undefined): string {
  switch ((executionKind ?? '').trim()) {
    case 'managed_http':
      return t('settingsAutomation.executionKindManagedHttp')
    case 'subprocess_cli':
      return t('settingsAutomation.executionKindSubprocessCli')
    default:
      return t('settingsAutomation.executionKindDirectHttp')
  }
}

export function placementKindLabel(t: TFunction, placementKind: string | null | undefined): string {
  switch ((placementKind ?? '').trim()) {
    case 'self_hosted':
      return t('settingsAutomation.placementSelfHosted')
    case 'installed_cli':
      return t('settingsAutomation.placementInstalledCli')
    case 'custom_hosted':
      return t('settingsAutomation.placementCustomHosted')
    default:
      return t('settingsAutomation.placementProviderHosted')
  }
}

export function placementKindDescription(t: TFunction, placementKind: string | null | undefined): string {
  switch ((placementKind ?? '').trim()) {
    case 'self_hosted':
      return t('settingsAutomation.placementDescriptionSelfHosted')
    case 'installed_cli':
      return t('settingsAutomation.placementDescriptionInstalledCli')
    case 'custom_hosted':
      return t('settingsAutomation.placementDescriptionCustomHosted')
    default:
      return t('settingsAutomation.placementDescriptionProviderHosted')
  }
}

export function requirementLabel(t: TFunction, requirement: string): string {
  if (requirement === 'os_secret_store') {
    return t('settingsAutomation.requirementOsSecretStore')
  }
  if (requirement.startsWith('local_server:')) {
    return t('settingsAutomation.requirementLocalServer', { service: requirement.slice(13) })
  }
  if (requirement.startsWith('cli:')) {
    return t('settingsAutomation.requirementCli', { tool: requirement.slice(4) })
  }
  if (requirement.startsWith('local_service:')) {
    return t('settingsAutomation.requirementLocalService', { service: requirement.slice(14) })
  }
  if (requirement.startsWith('endpoint:')) {
    return t('settingsAutomation.requirementEndpoint', { target: requirement.slice(9) })
  }
  return requirement
}

export function surfaceAuthScheme(
  surface: ProviderSurfaceSpec | undefined,
  endpointKind: EndpointSurfaceKind,
): string | null {
  if (!surface) return null
  const transport = endpointKind === 'ocr_api' ? surface.ocr_transport : surface.llm_transport
  return transport?.auth_scheme ?? null
}

export function surfaceUsesNoAuth(
  surface: ProviderSurfaceSpec | undefined,
  endpointKind: EndpointSurfaceKind,
): boolean {
  return surfaceAuthScheme(surface, endpointKind) === 'none'
}
