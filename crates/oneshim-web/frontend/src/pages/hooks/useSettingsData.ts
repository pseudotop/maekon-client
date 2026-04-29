import { useQuery } from '@tanstack/react-query'
import { useCallback, useDeferredValue } from 'react'
import {
  type AppSettings,
  type DesktopPermissionSnapshot,
  type ExternalApiSettings,
  type FeatureCapabilitySnapshot,
  fetchDesktopPermissionStatus,
  fetchFeatureCapabilities,
  fetchProviderSurfaces,
  fetchSecretBackendCapabilities,
  fetchSettings,
  fetchStorageStats,
  fetchUpdateStatus,
  type ProviderEndpointProbeResult,
  type ProviderSurfaceCatalog,
  type ProviderSurfaceSpec,
  probeProviderSurfaceEndpoint,
  type SecretBackendCapabilities,
  type StorageStats,
  type UpdateStatus,
} from '../../api/client'
import { DEFAULT_PROVIDER_SURFACE_CATALOG } from '../../api/defaultProviderSurfaceCatalog'
import { isStandaloneModeEnabled } from '../../api/standalone'
import {
  defaultSurfaceEndpoint,
  defaultSurfaceModel,
  deriveDefaultProviderSurfaceId,
  type EndpointSurfaceKind,
  providerSurfaceById,
  resolveProviderTypeForSurface,
  surfaceCompatibleWithAccessMode,
  surfaceKnownModel,
  surfaceSupportsModelSelection,
} from '../../features/providerSurfaces'
import { backendAllowsSecretEditing, supportsProjectionFor } from '../settings-utils'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface SettingsDataResult {
  // Raw query data
  settings: AppSettings | undefined
  settingsLoading: boolean
  storageStats: StorageStats | undefined
  storageLoading: boolean
  updateStatus: UpdateStatus | undefined
  featureCapabilities: FeatureCapabilitySnapshot | undefined
  desktopPermissionStatus: DesktopPermissionSnapshot | undefined
  desktopPermissionStatusError: string | null
  desktopPermissionStatusLoading: boolean
  desktopPermissionStatusRefreshing: boolean
  secretBackendCapabilities: SecretBackendCapabilities | undefined
  canQueryDesktopCapabilities: boolean

  // Provider catalog
  providerCatalog: ProviderSurfaceCatalog

  // Endpoint probe results
  ocrEndpointProbe: ProviderEndpointProbeResult | null
  ocrEndpointProbeLoading: boolean
  llmEndpointProbe: ProviderEndpointProbeResult | null
  llmEndpointProbeLoading: boolean

  // Derived defaults
  defaultByokBackendKind: string

  // Helpers
  handleRefreshDesktopPermissionStatus: () => void
  requestNotificationPermissionRefetch: () => void
  deriveEndpointAuthMode: (
    accessMode: string,
    endpointKind: EndpointSurfaceKind,
    surface: ProviderSurfaceSpec | undefined,
  ) => string
  deriveEndpointBackendKind: (authMode: string) => string
  defaultExternalApiSettings: (accessMode: string, endpointKind: EndpointSurfaceKind) => ExternalApiSettings
  normalizeEndpointSettings: (
    accessMode: string,
    endpointKind: EndpointSurfaceKind,
    endpoint: ExternalApiSettings | null | undefined,
    requestedSurface?: ProviderSurfaceSpec,
  ) => ExternalApiSettings | null
  resolveSurfaceForState: (state: AppSettings | null, which: 'ocr_api' | 'llm_api') => ProviderSurfaceSpec | undefined
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

export function useSettingsData(formData: AppSettings | null): SettingsDataResult {
  // Runtime detection rather than the module-level IS_TAURI const.
  // IS_TAURI is evaluated once at module load; in some DMG/packaged webview
  // builds the first evaluation can miss `__TAURI_INTERNALS__` before the
  // injection completes, permanently hiding the desktop permission section
  // for the entire session. Checking at render time (after React has mounted)
  // is reliable because Tauri always attaches the internals before user code.
  const canQueryDesktopCapabilities =
    typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window && !isStandaloneModeEnabled()

  // ---- Core settings query ------------------------------------------------
  const { data: settings, isLoading: settingsLoading } = useQuery({
    queryKey: ['settings'],
    queryFn: fetchSettings,
    staleTime: Number.POSITIVE_INFINITY,
  })

  // ---- Storage stats ------------------------------------------------------
  const { data: storageStats, isLoading: storageLoading } = useQuery({
    queryKey: ['storage-stats'],
    queryFn: fetchStorageStats,
    staleTime: 60_000,
  })

  // ---- Update status ------------------------------------------------------
  const { data: updateStatus } = useQuery<UpdateStatus>({
    queryKey: ['update-status'],
    queryFn: fetchUpdateStatus,
    refetchInterval: 15000,
    retry: 1,
  })

  // ---- Provider surface catalog -------------------------------------------
  const { data: providerSurfaceCatalog } = useQuery({
    queryKey: ['ai-provider-surfaces'],
    queryFn: fetchProviderSurfaces,
    staleTime: 300_000,
    retry: 1,
  })

  // ---- Feature capabilities -----------------------------------------------
  const { data: featureCapabilities } = useQuery<FeatureCapabilitySnapshot>({
    queryKey: ['feature-capabilities'],
    queryFn: fetchFeatureCapabilities,
    staleTime: 300_000,
    enabled: canQueryDesktopCapabilities,
    retry: 1,
  })

  // ---- Desktop permission status ------------------------------------------
  const {
    data: desktopPermissionStatus,
    error: desktopPermissionStatusError,
    isFetching: desktopPermissionStatusFetching,
    isLoading: desktopPermissionStatusLoading,
    refetch: refetchDesktopPermissionStatus,
  } = useQuery<DesktopPermissionSnapshot, Error>({
    queryKey: ['desktop-permission-status'],
    queryFn: fetchDesktopPermissionStatus,
    staleTime: 0,
    enabled: canQueryDesktopCapabilities,
    refetchOnReconnect: true,
    refetchOnWindowFocus: true,
    retry: 1,
  })

  const handleRefreshDesktopPermissionStatus = useCallback(() => {
    void refetchDesktopPermissionStatus()
  }, [refetchDesktopPermissionStatus])

  // ---- Secret backend capabilities ----------------------------------------
  const { data: secretBackendCapabilities } = useQuery({
    queryKey: ['secret-backend-capabilities'],
    queryFn: fetchSecretBackendCapabilities,
    staleTime: 300_000,
    enabled: canQueryDesktopCapabilities,
    retry: 1,
  })

  // ---- Provider catalog (resolved) ----------------------------------------
  const providerCatalog = providerSurfaceCatalog ?? DEFAULT_PROVIDER_SURFACE_CATALOG
  const defaultByokBackendKind = secretBackendCapabilities?.byok_backend_kind ?? 'unavailable'

  // ---- Derived helpers (memoised) -----------------------------------------
  const deriveEndpointAuthMode = useCallback(
    (accessMode: string, endpointKind: EndpointSurfaceKind, surface: ProviderSurfaceSpec | undefined): string => {
      if (surface?.execution_kind === 'managed_http') return 'managed_oauth'
      if (surface?.execution_kind === 'subprocess_cli') return 'cli_bridge'
      if (accessMode === 'ProviderOAuth' && endpointKind === 'llm_api') return 'managed_oauth'
      if (accessMode === 'ProviderSubscriptionCli' && endpointKind === 'llm_api') return 'cli_bridge'
      return 'api_key'
    },
    [],
  )

  const deriveEndpointBackendKind = useCallback(
    (authMode: string): string => {
      switch (authMode) {
        case 'managed_oauth':
          return 'os_secret_store'
        case 'cli_bridge':
          return 'bridge_managed'
        default:
          return defaultByokBackendKind
      }
    },
    [defaultByokBackendKind],
  )

  const defaultExternalApiSettings = useCallback(
    (accessMode: string, endpointKind: EndpointSurfaceKind): ExternalApiSettings => {
      const surfaceId = deriveDefaultProviderSurfaceId(
        providerCatalog,
        accessMode,
        endpointKind,
        'Generic',
        featureCapabilities,
      )
      const surface = providerSurfaceById(providerCatalog, surfaceId)
      const authMode = deriveEndpointAuthMode(accessMode, endpointKind, surface)
      const backendKind = deriveEndpointBackendKind(authMode)

      return {
        endpoint: defaultSurfaceEndpoint(surface, endpointKind),
        api_key_masked: '',
        model: defaultSurfaceModel(surface, endpointKind),
        provider_type: surface?.provider_type ?? 'Generic',
        surface_id: surfaceId,
        timeout_secs: 30,
        auth_mode: authMode,
        backend_kind: backendKind,
        has_secret: false,
        can_edit_secret: backendAllowsSecretEditing(backendKind) && authMode === 'api_key',
        secret_display_hint: null,
        projection_enabled: false,
      }
    },
    [deriveEndpointAuthMode, deriveEndpointBackendKind, featureCapabilities, providerCatalog],
  )

  const normalizeEndpointSettings = useCallback(
    (
      accessMode: string,
      endpointKind: EndpointSurfaceKind,
      endpoint: ExternalApiSettings | null | undefined,
      requestedSurface?: ProviderSurfaceSpec,
    ): ExternalApiSettings | null => {
      const seed = endpoint ?? defaultExternalApiSettings(accessMode, endpointKind)
      if (!seed) return null

      const seedProviderType = resolveProviderTypeForSurface(
        providerCatalog,
        requestedSurface?.surface_id ?? seed.surface_id,
        requestedSurface?.provider_type ?? seed.provider_type,
      )
      const previousSurface = providerSurfaceById(providerCatalog, seed.surface_id)
      const preservedSurface =
        !requestedSurface && surfaceCompatibleWithAccessMode(previousSurface, accessMode, endpointKind)
          ? previousSurface
          : undefined
      const surfaceId =
        requestedSurface?.surface_id ??
        preservedSurface?.surface_id ??
        deriveDefaultProviderSurfaceId(providerCatalog, accessMode, endpointKind, seedProviderType, featureCapabilities)
      const nextSurface = requestedSurface ?? preservedSurface ?? providerSurfaceById(providerCatalog, surfaceId)
      const providerType = resolveProviderTypeForSurface(
        providerCatalog,
        nextSurface?.surface_id ?? surfaceId,
        nextSurface?.provider_type ?? seedProviderType,
      )
      const previousDefaultEndpoint = defaultSurfaceEndpoint(previousSurface, endpointKind)
      const nextDefaultEndpoint = defaultSurfaceEndpoint(nextSurface, endpointKind)
      const previousDefaultModel = defaultSurfaceModel(previousSurface, endpointKind)
      const nextDefaultModel = defaultSurfaceModel(nextSurface, endpointKind)
      const supportsModelSel = surfaceSupportsModelSelection(nextSurface, endpointKind)
      const previousProviderType = resolveProviderTypeForSurface(providerCatalog, seed.surface_id, seed.provider_type)
      const authMode = deriveEndpointAuthMode(accessMode, endpointKind, nextSurface)
      const backendKind = deriveEndpointBackendKind(authMode)
      const currentModel = seed.model?.trim() ?? ''
      const knownNextModel = surfaceKnownModel(nextSurface, currentModel)
      const knownNextModelSupported = knownNextModel
        ? endpointKind === 'ocr_api'
          ? knownNextModel.capabilities.ocr && knownNextModel.capabilities.image_input
          : knownNextModel.capabilities.llm
        : null
      const providerChanged = previousProviderType !== providerType
      const executionChanged = previousSurface?.execution_kind !== nextSurface?.execution_kind
      const shouldResetModelForSurfaceChange =
        Boolean(currentModel) &&
        previousSurface?.surface_id !== nextSurface?.surface_id &&
        (knownNextModelSupported === false || (!knownNextModel && (providerChanged || executionChanged)))

      return {
        ...seed,
        endpoint:
          !seed.endpoint.trim() || seed.endpoint === previousDefaultEndpoint ? nextDefaultEndpoint : seed.endpoint,
        model: !supportsModelSel
          ? null
          : shouldResetModelForSurfaceChange
            ? nextDefaultModel
            : !seed.model?.trim() || seed.model === previousDefaultModel
              ? nextDefaultModel
              : seed.model,
        provider_type: providerType,
        surface_id: surfaceId,
        auth_mode: authMode,
        backend_kind: backendKind,
        can_edit_secret: backendAllowsSecretEditing(backendKind) && authMode === 'api_key',
        projection_enabled: supportsProjectionFor(authMode, backendKind) ? seed.projection_enabled : false,
      }
    },
    [
      defaultExternalApiSettings,
      deriveEndpointAuthMode,
      deriveEndpointBackendKind,
      featureCapabilities,
      providerCatalog,
    ],
  )

  // ---- Surface resolution helper ------------------------------------------
  const resolveSurfaceForState = useCallback(
    (state: AppSettings | null, which: 'ocr_api' | 'llm_api'): ProviderSurfaceSpec | undefined => {
      const endpoint = state?.ai_provider[which]
      const providerType = resolveProviderTypeForSurface(providerCatalog, endpoint?.surface_id, endpoint?.provider_type)
      const surfaceId =
        endpoint?.surface_id ??
        deriveDefaultProviderSurfaceId(
          providerCatalog,
          state?.ai_provider.access_mode,
          which,
          providerType,
          featureCapabilities,
        )
      return providerSurfaceById(providerCatalog, surfaceId)
    },
    [featureCapabilities, providerCatalog],
  )

  // ---- Endpoint probes ----------------------------------------------------
  const currentOcrSurface = resolveSurfaceForState(formData, 'ocr_api')
  const currentLlmSurface = resolveSurfaceForState(formData, 'llm_api')
  const deferredOcrEndpoint = useDeferredValue(formData?.ai_provider.ocr_api?.endpoint?.trim() ?? '')
  const deferredLlmEndpoint = useDeferredValue(formData?.ai_provider.llm_api?.endpoint?.trim() ?? '')

  const shouldProbeProviderEndpoint = (
    surface: ProviderSurfaceSpec | undefined,
    endpoint: string,
  ): surface is ProviderSurfaceSpec =>
    canQueryDesktopCapabilities &&
    Boolean(surface?.availability_probe) &&
    surface?.execution_kind === 'direct_http' &&
    (surface.placement_kind === 'self_hosted' || surface.placement_kind === 'custom_hosted') &&
    endpoint.length > 0

  const { data: ocrEndpointProbe, isFetching: ocrEndpointProbeLoading } = useQuery<ProviderEndpointProbeResult>({
    queryKey: ['provider-endpoint-probe', 'ocr_api', currentOcrSurface?.surface_id ?? null, deferredOcrEndpoint],
    queryFn: () => {
      if (!currentOcrSurface) {
        throw new Error('OCR endpoint probe requested without an active surface')
      }
      return probeProviderSurfaceEndpoint({
        surface_id: currentOcrSurface.surface_id,
        endpoint_kind: 'ocr_api',
        endpoint: deferredOcrEndpoint,
      })
    },
    enabled: shouldProbeProviderEndpoint(currentOcrSurface, deferredOcrEndpoint),
    retry: 0,
  })

  const { data: llmEndpointProbe, isFetching: llmEndpointProbeLoading } = useQuery<ProviderEndpointProbeResult>({
    queryKey: ['provider-endpoint-probe', 'llm_api', currentLlmSurface?.surface_id ?? null, deferredLlmEndpoint],
    queryFn: () => {
      if (!currentLlmSurface) {
        throw new Error('LLM endpoint probe requested without an active surface')
      }
      return probeProviderSurfaceEndpoint({
        surface_id: currentLlmSurface.surface_id,
        endpoint_kind: 'llm_api',
        endpoint: deferredLlmEndpoint,
      })
    },
    enabled: shouldProbeProviderEndpoint(currentLlmSurface, deferredLlmEndpoint),
    retry: 0,
  })

  // ---- Notification permission (expose refetch for mutation) ---------------
  const requestNotificationPermissionRefetch = handleRefreshDesktopPermissionStatus

  // ---- Return value -------------------------------------------------------
  return {
    settings,
    settingsLoading,
    storageStats,
    storageLoading,
    updateStatus,
    featureCapabilities,
    desktopPermissionStatus,
    desktopPermissionStatusError: desktopPermissionStatusError?.message ?? null,
    desktopPermissionStatusLoading,
    desktopPermissionStatusRefreshing: desktopPermissionStatusFetching && !desktopPermissionStatusLoading,
    secretBackendCapabilities,
    canQueryDesktopCapabilities,
    providerCatalog,
    ocrEndpointProbe: ocrEndpointProbe ?? null,
    ocrEndpointProbeLoading,
    llmEndpointProbe: llmEndpointProbe ?? null,
    llmEndpointProbeLoading,
    defaultByokBackendKind,
    handleRefreshDesktopPermissionStatus,
    requestNotificationPermissionRefetch,
    deriveEndpointAuthMode,
    deriveEndpointBackendKind,
    defaultExternalApiSettings,
    normalizeEndpointSettings,
    resolveSurfaceForState,
  }
}
