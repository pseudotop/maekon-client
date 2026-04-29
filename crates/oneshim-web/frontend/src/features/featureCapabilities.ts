import type {
  FeatureAvailability,
  FeatureCapability,
  FeatureCapabilitySnapshot,
  FeatureMaturity,
  ProviderSurfaceSpec,
} from '../api/contracts'

export function findFeatureCapability(
  snapshot: FeatureCapabilitySnapshot | null | undefined,
  featureId: string,
): FeatureCapability | null {
  return snapshot?.features.find((feature) => feature.feature_id === featureId) ?? null
}

export function maturityBadgeColor(
  maturity: FeatureCapability['maturity'],
): 'success' | 'warning' | 'error' | 'default' {
  switch (maturity) {
    case 'stable':
      return 'success'
    case 'beta':
      return 'warning'
    case 'experimental':
      return 'error'
    case 'deprecated':
      return 'default'
  }
}

function surfaceStabilityToMaturity(stability: string | null | undefined): FeatureMaturity {
  switch ((stability ?? '').trim().toLowerCase()) {
    case 'ga':
      return 'stable'
    case 'preview':
      return 'beta'
    case 'deprecated':
      return 'deprecated'
    default:
      return 'experimental'
  }
}

export function providerSurfaceAvailability(
  surface: ProviderSurfaceSpec | null | undefined,
  snapshot: FeatureCapabilitySnapshot | null | undefined,
): FeatureAvailability {
  if (!surface) {
    return 'unavailable'
  }

  const feature = findFeatureCapability(snapshot, surface.surface_id)
  if (feature) {
    return feature.availability
  }

  if (surface.execution_kind === 'direct_http') {
    if (surface.placement_kind === 'self_hosted' || surface.placement_kind === 'custom_hosted') {
      return 'partially_available'
    }
    return 'available'
  }

  return 'partially_available'
}

export function providerSurfaceMaturity(
  surface: ProviderSurfaceSpec | null | undefined,
  snapshot: FeatureCapabilitySnapshot | null | undefined,
): FeatureMaturity {
  if (!surface) {
    return 'deprecated'
  }

  return findFeatureCapability(snapshot, surface.surface_id)?.maturity ?? surfaceStabilityToMaturity(surface.stability)
}

export function providerSurfaceStatusCopyKey(
  surface: ProviderSurfaceSpec | null | undefined,
  snapshot: FeatureCapabilitySnapshot | null | undefined,
): string | null {
  if (!surface) {
    return null
  }

  return findFeatureCapability(snapshot, surface.surface_id)?.status_copy_key ?? null
}
