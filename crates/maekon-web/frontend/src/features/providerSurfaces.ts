import type {
  FeatureAvailability,
  FeatureCapabilitySnapshot,
  ProviderKnownModelSpec,
  ProviderSurfaceCatalog,
  ProviderSurfaceSpec,
  ProviderVendorSpec,
} from '../api/contracts'
import { providerSurfaceAvailability } from './featureCapabilities'

export type EndpointSurfaceKind = 'ocr_api' | 'llm_api'
export type UnknownModelPolicy = 'allow' | 'warn' | 'reject'
export type OcrExecutionStrategy = 'none' | 'multimodal_llm' | 'vision_api'

const STABILITY_RANK: Record<string, number> = {
  ga: 3,
  preview: 2,
  experimental: 1,
  deprecated: 0,
}

const PLACEMENT_RANK: Record<string, number> = {
  self_hosted: 3,
  installed_cli: 2,
  provider_hosted: 1,
  custom_hosted: 0,
}

const AVAILABILITY_RANK: Record<FeatureAvailability, number> = {
  available: 2,
  partially_available: 1,
  unavailable: 0,
}

function canonicalVendorSpec(
  catalog: ProviderSurfaceCatalog,
  rawProvider: string | null | undefined,
): ProviderVendorSpec | undefined {
  const normalized = (rawProvider ?? '').trim().toLowerCase()
  if (!normalized) {
    return catalog.vendors.find((vendor) => vendor.vendor_id.toLowerCase() === 'generic')
  }

  return catalog.vendors.find((vendor) => {
    return (
      vendor.vendor_id.toLowerCase() === normalized ||
      vendor.provider_type.toLowerCase() === normalized ||
      vendor.aliases.some((alias) => alias.toLowerCase() === normalized)
    )
  })
}

function normalizedProviderType(catalog: ProviderSurfaceCatalog, providerType: string | null | undefined): string {
  return canonicalVendorSpec(catalog, providerType)?.provider_type ?? ((providerType ?? '').trim() || 'Generic')
}

function normalizedVendorId(catalog: ProviderSurfaceCatalog, providerType: string | null | undefined): string {
  return (
    canonicalVendorSpec(catalog, providerType)?.vendor_id ?? ((providerType ?? '').trim().toLowerCase() || 'generic')
  )
}

function compatibleExecutionKinds(accessMode: string | null | undefined, endpointKind: EndpointSurfaceKind): string[] {
  if (endpointKind === 'ocr_api') {
    if (accessMode === 'ProviderSubscriptionCli') {
      return ['subprocess_cli', 'direct_http']
    }

    if (accessMode === 'ProviderOAuth') {
      return ['managed_http', 'direct_http']
    }

    return ['direct_http']
  }

  if (accessMode === 'ProviderSubscriptionCli') {
    return ['subprocess_cli']
  }

  if (accessMode === 'ProviderOAuth') {
    return ['managed_http', 'direct_http']
  }

  return ['direct_http']
}

function executionKindPriority(
  accessMode: string | null | undefined,
  endpointKind: EndpointSurfaceKind,
  executionKind: string,
): number {
  const ordered = compatibleExecutionKinds(accessMode, endpointKind)
  const index = ordered.indexOf(executionKind)
  return index >= 0 ? index : ordered.length
}

function surfaceSupportsKind(surface: ProviderSurfaceSpec, endpointKind: EndpointSurfaceKind): boolean {
  return endpointKind === 'ocr_api' ? surface.supports.ocr : surface.supports.llm
}

function featureAvailabilityScore(
  surface: ProviderSurfaceSpec,
  snapshot: FeatureCapabilitySnapshot | null | undefined,
): number {
  return AVAILABILITY_RANK[providerSurfaceAvailability(surface, snapshot)]
}

function compareProviderSurfaces(
  left: ProviderSurfaceSpec,
  right: ProviderSurfaceSpec,
  snapshot?: FeatureCapabilitySnapshot | null,
): number {
  const availabilityDelta = featureAvailabilityScore(right, snapshot) - featureAvailabilityScore(left, snapshot)
  if (availabilityDelta !== 0) {
    return availabilityDelta
  }

  if (left.preferred_for_product_auth !== right.preferred_for_product_auth) {
    return Number(right.preferred_for_product_auth) - Number(left.preferred_for_product_auth)
  }

  const stabilityDelta = (STABILITY_RANK[right.stability] ?? -1) - (STABILITY_RANK[left.stability] ?? -1)
  if (stabilityDelta !== 0) {
    return stabilityDelta
  }

  const placementDelta = (PLACEMENT_RANK[right.placement_kind] ?? -1) - (PLACEMENT_RANK[left.placement_kind] ?? -1)
  if (placementDelta !== 0) {
    return placementDelta
  }

  return left.display_name.localeCompare(right.display_name)
}

function compareOcrDefaultSurfaces(
  left: ProviderSurfaceSpec,
  right: ProviderSurfaceSpec,
  snapshot?: FeatureCapabilitySnapshot | null,
): number {
  const directHttpDelta = Number(right.execution_kind === 'direct_http') - Number(left.execution_kind === 'direct_http')
  if (directHttpDelta !== 0) {
    return directHttpDelta
  }

  return compareProviderSurfaces(left, right, snapshot)
}

export function sortProviderSurfaces(
  surfaces: ProviderSurfaceSpec[],
  snapshot?: FeatureCapabilitySnapshot | null,
): ProviderSurfaceSpec[] {
  return [...surfaces].sort((left, right) => compareProviderSurfaces(left, right, snapshot))
}

export function getCompatibleProviderSurfaces(
  catalog: ProviderSurfaceCatalog,
  accessMode: string | null | undefined,
  endpointKind: EndpointSurfaceKind,
  snapshot?: FeatureCapabilitySnapshot | null,
): ProviderSurfaceSpec[] {
  const executionKinds = compatibleExecutionKinds(accessMode, endpointKind)

  const compatible = catalog.surfaces.filter(
    (surface) => executionKinds.includes(surface.execution_kind) && surfaceSupportsKind(surface, endpointKind),
  )

  return [...compatible].sort((left, right) => {
    const priorityDelta =
      executionKindPriority(accessMode, endpointKind, left.execution_kind) -
      executionKindPriority(accessMode, endpointKind, right.execution_kind)
    if (priorityDelta !== 0) {
      return priorityDelta
    }
    return endpointKind === 'ocr_api'
      ? compareOcrDefaultSurfaces(left, right, snapshot)
      : compareProviderSurfaces(left, right, snapshot)
  })
}

export function surfaceCompatibleWithAccessMode(
  surface: ProviderSurfaceSpec | null | undefined,
  accessMode: string | null | undefined,
  endpointKind: EndpointSurfaceKind,
): boolean {
  if (!surface) {
    return false
  }

  return (
    compatibleExecutionKinds(accessMode, endpointKind).includes(surface.execution_kind) &&
    surfaceSupportsKind(surface, endpointKind)
  )
}

export function deriveDefaultProviderSurfaceId(
  catalog: ProviderSurfaceCatalog,
  accessMode: string | null | undefined,
  endpointKind: EndpointSurfaceKind,
  providerType: string | null | undefined,
  snapshot?: FeatureCapabilitySnapshot | null,
): string | null {
  const normalizedVendor = normalizedVendorId(catalog, providerType)
  const compatible = getCompatibleProviderSurfaces(catalog, accessMode, endpointKind, snapshot)
  const vendorMatch = compatible.filter((surface) => surface.vendor_id.toLowerCase() === normalizedVendor)
  const rawCandidates = vendorMatch.length > 0 ? vendorMatch : compatible
  const candidates = [...rawCandidates].sort((left, right) => {
    const priorityDelta =
      executionKindPriority(accessMode, endpointKind, left.execution_kind) -
      executionKindPriority(accessMode, endpointKind, right.execution_kind)
    if (priorityDelta !== 0) {
      return priorityDelta
    }
    return endpointKind === 'ocr_api'
      ? compareOcrDefaultSurfaces(left, right, snapshot)
      : compareProviderSurfaces(left, right, snapshot)
  })

  return candidates[0]?.surface_id ?? null
}

export function providerSurfaceById(
  catalog: ProviderSurfaceCatalog,
  surfaceId: string | null | undefined,
): ProviderSurfaceSpec | undefined {
  return providerSurfaceByIdFromList(catalog.surfaces, surfaceId)
}

export function providerSurfaceByIdFromList(
  surfaces: ProviderSurfaceSpec[],
  surfaceId: string | null | undefined,
): ProviderSurfaceSpec | undefined {
  const normalized = (surfaceId ?? '').trim().toLowerCase()
  if (!normalized) {
    return undefined
  }

  return surfaces.find((surface) => surface.surface_id.toLowerCase() === normalized)
}

export function relatedProviderSurfaces(
  catalog: ProviderSurfaceCatalog,
  surface: ProviderSurfaceSpec | null | undefined,
): ProviderSurfaceSpec[] {
  return relatedProviderSurfacesFromList(catalog.surfaces, surface)
}

export function relatedProviderSurfacesFromList(
  surfaces: ProviderSurfaceSpec[],
  surface: ProviderSurfaceSpec | null | undefined,
): ProviderSurfaceSpec[] {
  if (!surface) {
    return []
  }

  return (surface.related_surface_ids ?? [])
    .map((surfaceId) => providerSurfaceByIdFromList(surfaces, surfaceId))
    .filter((candidate): candidate is ProviderSurfaceSpec => candidate != null)
}

export function preferredRelatedProviderSurface(
  catalog: ProviderSurfaceCatalog,
  surface: ProviderSurfaceSpec | null | undefined,
  executionKind?: string,
  snapshot?: FeatureCapabilitySnapshot | null,
): ProviderSurfaceSpec | undefined {
  return preferredRelatedProviderSurfaceFromList(catalog.surfaces, surface, executionKind, snapshot)
}

export function preferredRelatedProviderSurfaceFromList(
  surfaces: ProviderSurfaceSpec[],
  surface: ProviderSurfaceSpec | null | undefined,
  executionKind?: string,
  snapshot?: FeatureCapabilitySnapshot | null,
): ProviderSurfaceSpec | undefined {
  const related = relatedProviderSurfacesFromList(surfaces, surface).filter(
    (candidate) => executionKind == null || candidate.execution_kind === executionKind,
  )

  return sortProviderSurfaces(related, snapshot)[0]
}

export function resolveProviderTypeForSurface(
  catalog: ProviderSurfaceCatalog,
  surfaceId: string | null | undefined,
  fallbackProviderType?: string | null,
): string {
  return providerSurfaceById(catalog, surfaceId)?.provider_type ?? normalizedProviderType(catalog, fallbackProviderType)
}

export function defaultSurfaceEndpoint(
  surface: ProviderSurfaceSpec | undefined,
  endpointKind: EndpointSurfaceKind,
): string {
  if (!surface) {
    return ''
  }

  const transport = endpointKind === 'ocr_api' ? surface.ocr_transport : surface.llm_transport
  return transport?.url ?? ''
}

export function defaultSurfaceModel(
  surface: ProviderSurfaceSpec | undefined,
  endpointKind: EndpointSurfaceKind,
): string | null {
  if (!surface) {
    return null
  }

  const models = endpointKind === 'ocr_api' ? surface.default_models.ocr_models : surface.default_models.llm_models
  return models[0] ?? null
}

export function surfaceSupportsParameter(
  surface: ProviderSurfaceSpec | undefined,
  endpointKind: EndpointSurfaceKind,
  parameter: string,
): boolean {
  if (!surface) {
    return false
  }

  const normalized = parameter.trim().toLowerCase()
  if (!normalized) {
    return false
  }

  const profile = endpointKind === 'ocr_api' ? surface.parameter_profiles.ocr : surface.parameter_profiles.llm
  return profile.supported.some((candidate) => candidate.toLowerCase() === normalized)
}

export function surfaceSupportsModelSelection(
  surface: ProviderSurfaceSpec | undefined,
  endpointKind: EndpointSurfaceKind,
): boolean {
  if (!surface) {
    return false
  }

  const defaults = endpointKind === 'ocr_api' ? surface.default_models.ocr_models : surface.default_models.llm_models
  const modelCatalogSupported =
    endpointKind === 'ocr_api'
      ? (surface.model_catalog_transport?.ocr_supported ?? false)
      : (surface.model_catalog_transport?.llm_supported ?? false)
  const knownModels = surface.known_models.some((model) =>
    endpointKind === 'ocr_api' ? model.capabilities.ocr : model.capabilities.llm,
  )

  return defaults.length > 0 || modelCatalogSupported || knownModels
}

function matchesKnownModel(model: ProviderKnownModelSpec, value: string): boolean {
  const normalized = value.trim().toLowerCase()
  if (!normalized) {
    return false
  }

  if (model.id.toLowerCase() === normalized) {
    return true
  }

  if (model.aliases.some((alias) => alias.toLowerCase() === normalized)) {
    return true
  }

  return model.id_prefixes.some((prefix) => {
    const normalizedPrefix = prefix.trim().toLowerCase()
    return normalizedPrefix.length > 0 && (normalized === normalizedPrefix || normalized.startsWith(normalizedPrefix))
  })
}

export function surfaceKnownModel(
  surface: ProviderSurfaceSpec | undefined,
  model: string | null | undefined,
): ProviderKnownModelSpec | undefined {
  const candidate = (model ?? '').trim()
  if (!surface || !candidate) {
    return undefined
  }

  return surface.known_models.find((entry) => matchesKnownModel(entry, candidate))
}

export function surfaceModelSupportsCapability(
  surface: ProviderSurfaceSpec | undefined,
  endpointKind: EndpointSurfaceKind,
  model: string | null | undefined,
): boolean | null {
  const known = surfaceKnownModel(surface, model)
  if (!known) {
    return null
  }

  return endpointKind === 'ocr_api' ? known.capabilities.ocr : known.capabilities.llm
}

export function surfaceUnknownModelPolicy(
  surface: ProviderSurfaceSpec | undefined,
  endpointKind: EndpointSurfaceKind,
): UnknownModelPolicy {
  if (!surface) {
    return 'warn'
  }

  return endpointKind === 'ocr_api'
    ? (surface.unknown_model_policy?.ocr ?? 'warn')
    : (surface.unknown_model_policy?.llm ?? 'warn')
}

export function surfaceLlmStructuredOutput(surface: ProviderSurfaceSpec | undefined): boolean {
  return surface?.llm_capabilities?.structured_output ?? false
}

export function surfaceOcrExecutionStrategy(surface: ProviderSurfaceSpec | undefined): OcrExecutionStrategy {
  const strategy = surface?.ocr_capabilities?.strategy?.trim()
  if (strategy === 'multimodal_llm' || strategy === 'vision_api') {
    return strategy
  }
  return 'none'
}

export function surfaceOcrRequiresStructuredOutputModel(surface: ProviderSurfaceSpec | undefined): boolean {
  return surface?.ocr_capabilities?.requires_structured_output_model ?? false
}
