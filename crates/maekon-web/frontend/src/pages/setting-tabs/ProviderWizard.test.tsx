import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import ProviderWizard from './ProviderWizard'

describe('ProviderWizard', () => {
  it('uses bundled provider icon URLs that are compatible with the desktop CSP', () => {
    const { container } = renderWithProviders(<ProviderWizard onSelect={vi.fn()} />)

    const icons = Array.from(container.querySelectorAll('img'))

    expect(icons).toHaveLength(6)
    for (const icon of icons) {
      expect(icon.getAttribute('src')).not.toMatch(/^data:/)
    }
  })
})
