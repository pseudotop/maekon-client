import { fireEvent, screen, waitFor } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import LanguageSelector from '../LanguageSelector'

describe('LanguageSelector', () => {
  it('renders supported languages in a shared Select control', () => {
    renderWithProviders(<LanguageSelector />)

    const select = screen.getByRole('combobox')
    expect(select).toBeInTheDocument()
    expect(screen.getByRole('option', { name: 'English' })).toBeInTheDocument()
    expect(screen.getByRole('option', { name: '한국어' })).toBeInTheDocument()
    expect(screen.getByRole('option', { name: '日本語' })).toBeInTheDocument()
  })

  it('persists the selected language', async () => {
    renderWithProviders(<LanguageSelector />)

    fireEvent.change(screen.getByRole('combobox'), {
      target: { value: 'ko' },
    })

    await waitFor(() => {
      expect(localStorage.getItem('oneshim-language')).toBe('ko')
    })
  })
})
