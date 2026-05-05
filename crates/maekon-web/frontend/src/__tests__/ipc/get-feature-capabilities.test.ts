import { invoke } from '@tauri-apps/api/core'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

describe('CRT-MK-M051: get_feature_capabilities IPC contract', () => {
  afterEach(() => clearMocks())

  it('M051: returns feature capability snapshot', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_feature_capabilities') {
        return {
          features: [
            {
              feature_id: 'provider_surface.openai.managed_oauth',
              maturity: 'experimental',
              availability: 'available',
              preferred: false,
              requires: ['os_secret_store'],
              status_reason: null,
              status_copy_key: 'featureCapability.surface.provider_surface.openai.managed_oauth.available',
              setup_copy_key: null,
              setup_docs_url: null,
              configuration_env_vars: [],
            },
          ],
        }
      }
    })

    const result = await invoke<{
      features: Array<{
        feature_id: string
        maturity: string
        availability: string
        preferred: boolean
        requires: string[]
        status_reason: string | null
        status_copy_key: string | null
        setup_copy_key: string | null
        setup_docs_url: string | null
        configuration_env_vars: string[]
      }>
    }>('get_feature_capabilities')

    expect(result.features).toHaveLength(1)
    expect(result.features[0]?.feature_id).toBe('provider_surface.openai.managed_oauth')
    expect(result.features[0]?.maturity).toBe('experimental')
  })
})
