import { describe, expect, it } from 'vitest'
import type { ProviderSurfaceSpec } from '../../api/contracts'
import {
  findFeatureCapability,
  maturityBadgeColor,
  providerSurfaceAvailability,
  providerSurfaceCliDiscovery,
  providerSurfaceCliReadiness,
} from '../featureCapabilities'

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

  it('returns provider CLI readiness from the feature snapshot', () => {
    const surface = {
      surface_id: 'provider_surface.google.antigravity_cli',
    } as ProviderSurfaceSpec

    expect(
      providerSurfaceCliReadiness(surface, {
        features: [
          {
            feature_id: 'provider_surface.google.antigravity_cli',
            maturity: 'beta',
            availability: 'partially_available',
            provider_cli_readiness: 'auth_ready',
            preferred: false,
            requires: ['cli:antigravity'],
            status_reason: 'cli_auth_ready_runtime_unsupported',
            status_copy_key: 'featureCapability.surface.provider_surface.google.antigravity_cli.partially_available',
            setup_copy_key: null,
            setup_docs_url: null,
            configuration_env_vars: [],
          },
        ],
      }),
    ).toBe('auth_ready')
  })

  it('returns provider CLI discovery details from the feature snapshot', () => {
    const surface = {
      surface_id: 'provider_surface.anthropic.subprocess_cli',
    } as ProviderSurfaceSpec

    const discovery = providerSurfaceCliDiscovery(surface, {
      features: [
        {
          feature_id: 'provider_surface.anthropic.subprocess_cli',
          maturity: 'beta',
          availability: 'partially_available',
          provider_cli_discovery: {
            candidate_name: 'claude',
            executable_hint: 'claude.exe',
            version_status: 'not_checked',
            dependency_status: 'stale_process_env',
            status_reason: 'claude_code_git_bash_path_requires_restart',
            env_refresh_required: true,
          },
          preferred: true,
          requires: ['cli:claude-code'],
          status_reason: 'claude_code_git_bash_path_requires_restart',
          status_copy_key: 'featureCapability.surface.provider_surface.anthropic.subprocess_cli.partially_available',
          setup_copy_key: null,
          setup_docs_url: null,
          configuration_env_vars: [],
        },
      ],
    })

    expect(discovery?.env_refresh_required).toBe(true)
    expect(discovery?.dependency_status).toBe('stale_process_env')
  })
})
