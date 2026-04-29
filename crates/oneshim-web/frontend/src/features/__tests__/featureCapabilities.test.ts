import { describe, expect, it } from 'vitest'
import type { ProviderSurfaceSpec } from '../../api/contracts'
import { findFeatureCapability, maturityBadgeColor, providerSurfaceAvailability } from '../featureCapabilities'

describe('featureCapabilities helpers', () => {
  it('finds a feature by id', () => {
    const feature = findFeatureCapability(
      {
        features: [
          {
            feature_id: 'provider_surface.openai.managed_oauth',
            maturity: 'experimental',
            availability: 'available',
            preferred: false,
            requires: ['os_secret_store'],
            status_reason: null,
            status_copy_key: null,
            setup_copy_key: null,
            setup_docs_url: null,
            configuration_env_vars: [],
          },
        ],
      },
      'provider_surface.openai.managed_oauth',
    )

    expect(feature?.maturity).toBe('experimental')
  })

  it('maps maturity to badge color', () => {
    expect(maturityBadgeColor('stable')).toBe('success')
    expect(maturityBadgeColor('beta')).toBe('warning')
    expect(maturityBadgeColor('experimental')).toBe('error')
    expect(maturityBadgeColor('deprecated')).toBe('default')
  })

  it('treats self-hosted direct surfaces as partially available without a desktop probe', () => {
    const surface = {
      surface_id: 'provider_surface.ollama.local_http',
      execution_kind: 'direct_http',
      placement_kind: 'self_hosted',
    } as ProviderSurfaceSpec

    expect(providerSurfaceAvailability(surface, null)).toBe('partially_available')
  })
})
