import { fireEvent, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import ProviderWizard, { applyProviderWizardSelection, type ProviderDef } from './ProviderWizard'

describe('ProviderWizard', () => {
  it('uses bundled provider icon URLs that are compatible with the desktop CSP', () => {
    const { container } = renderWithProviders(<ProviderWizard onSelect={vi.fn()} />)

    const icons = Array.from(container.querySelectorAll('img'))

    expect(icons).toHaveLength(6)
    for (const icon of icons) {
      expect(icon.getAttribute('src')).not.toMatch(/^data:/)
    }
  })

  it('hands over the chosen provider and the trimmed key only once a key is entered (#13734)', () => {
    const onSelect = vi.fn()
    renderWithProviders(<ProviderWizard onSelect={onSelect} />)

    fireEvent.click(screen.getByTestId('provider-wizard-option-anthropic'))
    expect(screen.getByTestId('provider-wizard-save')).toBeDisabled()
    fireEvent.change(screen.getByTestId('provider-wizard-api-key'), { target: { value: '  sk-ant-test  ' } })
    fireEvent.click(screen.getByTestId('provider-wizard-save'))

    expect(onSelect).toHaveBeenCalledTimes(1)
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'anthropic', surfaceId: 'provider_surface.anthropic.direct_api' }),
      'sk-ant-test',
    )
  })
})

describe('applyProviderWizardSelection (#13734)', () => {
  function form() {
    return {
      handleAiProviderChange: vi.fn(),
      handleProviderSurfaceChange: vi.fn(),
      handleExternalApiChange: vi.fn(),
    }
  }

  function provider(overrides: Partial<ProviderDef> = {}): ProviderDef {
    return {
      id: 'example',
      name: 'Example',
      icon: '',
      tier: 'recommended',
      tierLabel: 'settings.ai.tierRecommended',
      apiKeyEnv: 'EXAMPLE_API_KEY',
      docsUrl: 'https://example.invalid/keys',
      placeholder: 'ex-...',
      surfaceId: 'provider_surface.example.direct_api',
      defaultModel: 'example-model',
      ...overrides,
    }
  }

  it('saves a hosted provider over the API-key path with its surface, key and default model', () => {
    const target = form()

    applyProviderWizardSelection(target, provider(), 'ex-key')

    expect(target.handleAiProviderChange.mock.calls).toEqual([
      ['access_mode', 'ProviderApiKey'],
      ['llm_provider', 'Remote'],
    ])
    expect(target.handleProviderSurfaceChange.mock.calls).toEqual([['llm_api', 'provider_surface.example.direct_api']])
    // The surface comes first: choosing it rewrites the endpoint and model, so the wizard's model lands after it.
    expect(target.handleExternalApiChange.mock.calls).toEqual([
      ['llm_api', 'api_key_masked', 'ex-key'],
      ['llm_api', 'model', 'example-model'],
    ])
  })

  it('keeps a local provider local and never writes an empty key', () => {
    const target = form()

    applyProviderWizardSelection(target, provider({ tier: 'local' }), '')

    expect(target.handleAiProviderChange).toHaveBeenCalledWith('llm_provider', 'Local')
    expect(target.handleExternalApiChange.mock.calls).toEqual([['llm_api', 'model', 'example-model']])
  })
})
