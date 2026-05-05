import { describe, expect, it } from 'vitest'
import type { FeatureCapabilitySnapshot, ProviderSurfaceCatalog, ProviderSurfaceSpec } from '../../api/contracts'
import { DEFAULT_PROVIDER_SURFACE_CATALOG } from '../../api/defaultProviderSurfaceCatalog'
import {
  deriveDefaultProviderSurfaceId,
  getCompatibleProviderSurfaces,
  preferredRelatedProviderSurface,
  resolveProviderTypeForSurface,
  sortProviderSurfaces,
  surfaceKnownModel,
  surfaceModelSupportsCapability,
  surfaceSupportsModelSelection,
} from '../providerSurfaces'

describe('provider surface defaults', () => {
  it('uses managed oauth for OpenAI llm oauth mode', () => {
    expect(deriveDefaultProviderSurfaceId(DEFAULT_PROVIDER_SURFACE_CATALOG, 'ProviderOAuth', 'llm_api', 'OpenAi')).toBe(
      'provider_surface.openai.managed_oauth',
    )
  })

  it('uses subprocess surface for subscription cli mode', () => {
    expect(
      deriveDefaultProviderSurfaceId(
        DEFAULT_PROVIDER_SURFACE_CATALOG,
        'ProviderSubscriptionCli',
        'llm_api',
        'Anthropic',
      ),
    ).toBe('provider_surface.anthropic.subprocess_cli')
  })

  it('keeps direct OCR surfaces available in subscription cli mode', () => {
    expect(
      deriveDefaultProviderSurfaceId(DEFAULT_PROVIDER_SURFACE_CATALOG, 'ProviderSubscriptionCli', 'ocr_api', 'OpenAi'),
    ).toBe('provider_surface.openai.subprocess_cli')
  })

  it('lists OCR-compatible direct surfaces even when llm access mode uses provider CLI', () => {
    const surfaces = getCompatibleProviderSurfaces(
      DEFAULT_PROVIDER_SURFACE_CATALOG,
      'ProviderSubscriptionCli',
      'ocr_api',
    )
    expect(surfaces.some((surface) => surface.surface_id === 'provider_surface.openai.subprocess_cli')).toBe(true)
    expect(surfaces.some((surface) => surface.surface_id === 'provider_surface.openai.direct_api')).toBe(true)
    expect(surfaces.some((surface) => surface.surface_id === 'provider_surface.anthropic.direct_api')).toBe(true)
  })

  it('allows future OCR subprocess surfaces only in compatible access modes', () => {
    const openAiDirect = DEFAULT_PROVIDER_SURFACE_CATALOG.surfaces.find(
      (surface) => surface.surface_id === 'provider_surface.openai.direct_api',
    )
    expect(openAiDirect).toBeDefined()

    const ocrCliSurface: ProviderSurfaceSpec = {
      ...openAiDirect!,
      surface_id: 'provider_surface.openai.ocr_subprocess_cli',
      display_name: 'OpenAI OCR CLI',
      execution_kind: 'subprocess_cli',
      placement_kind: 'installed_cli',
      credential_kind: 'cli_bridge',
      supports: {
        ...openAiDirect!.supports,
        ocr: true,
      },
      related_surface_ids: ['provider_surface.openai.subprocess_cli'],
      ocr_transport: null,
      subprocess_transport: {
        tool_id: 'codex',
        executable_candidates: ['codex'],
        auth_probe_command: ['login', 'status'],
        auth_probe_mode: 'codex_login_status_text',
        invocation_mode: 'codex_exec_json',
        model_flag: '--model',
        json_output_supported: true,
      },
    }

    const catalog: ProviderSurfaceCatalog = {
      ...DEFAULT_PROVIDER_SURFACE_CATALOG,
      surfaces: [...DEFAULT_PROVIDER_SURFACE_CATALOG.surfaces, ocrCliSurface],
    }

    expect(
      getCompatibleProviderSurfaces(catalog, 'ProviderApiKey', 'ocr_api').some(
        (surface) => surface.surface_id === ocrCliSurface.surface_id,
      ),
    ).toBe(false)
    expect(
      getCompatibleProviderSurfaces(catalog, 'ProviderSubscriptionCli', 'ocr_api').some(
        (surface) => surface.surface_id === ocrCliSurface.surface_id,
      ),
    ).toBe(true)
    expect(getCompatibleProviderSurfaces(catalog, 'ProviderSubscriptionCli', 'ocr_api')[0]?.surface_id).not.toBe(
      'provider_surface.openai.direct_api',
    )
  })

  it('falls back to direct api for generic provider types', () => {
    expect(
      deriveDefaultProviderSurfaceId(DEFAULT_PROVIDER_SURFACE_CATALOG, 'ProviderApiKey', 'llm_api', 'Generic'),
    ).toBe('provider_surface.generic.direct_api')
  })

  it('matches provider aliases from the vendor catalog when deriving default surfaces', () => {
    expect(
      deriveDefaultProviderSurfaceId(DEFAULT_PROVIDER_SURFACE_CATALOG, 'ProviderApiKey', 'llm_api', 'open_ai'),
    ).toBe('provider_surface.openai.direct_api')
    expect(
      deriveDefaultProviderSurfaceId(DEFAULT_PROVIDER_SURFACE_CATALOG, 'ProviderApiKey', 'llm_api', 'gemini'),
    ).toBe('provider_surface.google.direct_api')
    expect(
      deriveDefaultProviderSurfaceId(DEFAULT_PROVIDER_SURFACE_CATALOG, 'ProviderApiKey', 'llm_api', 'llamaindex'),
    ).toBe('provider_surface.generic.direct_api')
  })

  it('resolves fallback provider types through vendor aliases', () => {
    expect(resolveProviderTypeForSurface(DEFAULT_PROVIDER_SURFACE_CATALOG, null, 'open_ai')).toBe('OpenAi')
    expect(resolveProviderTypeForSurface(DEFAULT_PROVIDER_SURFACE_CATALOG, null, 'gemini')).toBe('Google')
    expect(resolveProviderTypeForSurface(DEFAULT_PROVIDER_SURFACE_CATALOG, null, 'llamaindex')).toBe('Generic')
  })

  it('filters compatible surfaces for oauth llm mode', () => {
    const surfaces = getCompatibleProviderSurfaces(DEFAULT_PROVIDER_SURFACE_CATALOG, 'ProviderOAuth', 'llm_api')
    const surfaceIds = surfaces.map((surface) => surface.surface_id)
    // Both OpenAI and Copilot OAuth surfaces must be present in the result.
    // Sort order is by availability → preferred → stability → placement → name,
    // so the leading surface depends on catalog metadata (not asserted here).
    expect(surfaceIds).toContain('provider_surface.openai.managed_oauth')
    expect(surfaces.some((surface) => surface.execution_kind === 'managed_http')).toBe(true)
    expect(surfaces.some((surface) => surface.execution_kind === 'direct_http')).toBe(true)
    // The first surface must be one of the managed_oauth options (sort places managed first).
    expect(surfaces[0]?.credential_kind).toBe('managed_oauth')
  })

  it('allows direct and managed OCR surfaces in oauth mode', () => {
    const surfaces = getCompatibleProviderSurfaces(DEFAULT_PROVIDER_SURFACE_CATALOG, 'ProviderOAuth', 'ocr_api')
    expect(surfaces.some((surface) => surface.execution_kind === 'direct_http')).toBe(true)
    expect(surfaces.some((surface) => surface.surface_id === 'provider_surface.google.managed_oauth')).toBe(true)
  })

  it('prefers higher-stability preferred surfaces within the same compatibility set', () => {
    const subprocess = DEFAULT_PROVIDER_SURFACE_CATALOG.surfaces.find(
      (surface) => surface.surface_id === 'provider_surface.openai.subprocess_cli',
    )
    expect(subprocess).toBeDefined()

    const legacySubprocess = {
      ...subprocess!,
      surface_id: 'provider_surface.test.legacy_cli',
      display_name: 'Legacy CLI',
      preferred_for_product_auth: false,
      stability: 'experimental',
    }

    expect(sortProviderSurfaces([legacySubprocess, subprocess!]).map((surface) => surface.surface_id)).toEqual([
      'provider_surface.openai.subprocess_cli',
      'provider_surface.test.legacy_cli',
    ])
  })

  it('resolves explicit related subprocess surface for managed oauth', () => {
    const oauthSurface = DEFAULT_PROVIDER_SURFACE_CATALOG.surfaces.find(
      (surface) => surface.surface_id === 'provider_surface.openai.managed_oauth',
    )

    expect(
      preferredRelatedProviderSurface(DEFAULT_PROVIDER_SURFACE_CATALOG, oauthSurface, 'subprocess_cli')?.surface_id,
    ).toBe('provider_surface.openai.subprocess_cli')
  })

  it('uses capability availability when deriving default CLI surface', () => {
    const snapshot: FeatureCapabilitySnapshot = {
      features: [
        {
          feature_id: 'provider_surface.openai.subprocess_cli',
          maturity: 'beta',
          availability: 'available',
          preferred: true,
          requires: ['cli:codex'],
          status_reason: null,
          status_copy_key: null,
          setup_copy_key: null,
          setup_docs_url: null,
          configuration_env_vars: [],
        },
        {
          feature_id: 'provider_surface.anthropic.subprocess_cli',
          maturity: 'beta',
          availability: 'unavailable',
          preferred: true,
          requires: ['cli:claude-code'],
          status_reason: null,
          status_copy_key: null,
          setup_copy_key: null,
          setup_docs_url: null,
          configuration_env_vars: [],
        },
      ],
    }

    expect(
      deriveDefaultProviderSurfaceId(
        DEFAULT_PROVIDER_SURFACE_CATALOG,
        'ProviderSubscriptionCli',
        'llm_api',
        'Generic',
        snapshot,
      ),
    ).toBe('provider_surface.openai.subprocess_cli')
  })

  it('treats self-hosted direct surfaces as unavailable when feature snapshot says so', () => {
    const snapshot: FeatureCapabilitySnapshot = {
      features: [
        {
          feature_id: 'provider_surface.ollama.local_http',
          maturity: 'stable',
          availability: 'unavailable',
          preferred: false,
          requires: ['local_service:ollama'],
          status_reason: 'service_unreachable',
          status_copy_key: 'featureCapability.surface.provider_surface.ollama.local_http.unavailable',
          setup_copy_key: null,
          setup_docs_url: null,
          configuration_env_vars: [],
        },
      ],
    }

    expect(
      deriveDefaultProviderSurfaceId(
        DEFAULT_PROVIDER_SURFACE_CATALOG,
        'ProviderApiKey',
        'llm_api',
        'Generic',
        snapshot,
      ),
    ).toBe('provider_surface.generic.direct_api')
  })

  it('derives model-selection support from surface catalog semantics', () => {
    const google = DEFAULT_PROVIDER_SURFACE_CATALOG.surfaces.find(
      (surface) => surface.surface_id === 'provider_surface.google.direct_api',
    )
    expect(surfaceSupportsModelSelection(google, 'ocr_api')).toBe(false)
    expect(surfaceSupportsModelSelection(google, 'llm_api')).toBe(true)
  })

  it('matches known Ollama model capabilities by prefix', () => {
    const ollama = DEFAULT_PROVIDER_SURFACE_CATALOG.surfaces.find(
      (surface) => surface.surface_id === 'provider_surface.ollama.local_http',
    )
    expect(surfaceKnownModel(ollama, 'qwen3-vl:8b-instruct-q4_K_M')?.id).toBe('qwen3-vl:8b')
    expect(surfaceModelSupportsCapability(ollama, 'ocr_api', 'qwen3-vl:8b-instruct-q4_K_M')).toBe(true)
    expect(surfaceModelSupportsCapability(ollama, 'ocr_api', 'qwen3:8b')).toBe(false)
  })
})
