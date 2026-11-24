import { act, fireEvent, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import en from '../i18n/locales/en.json'
import { SupportDetails } from './SupportDetails'

describe('SupportDetails (#13737)', () => {
  afterEach(() => {
    vi.useRealTimers()
    Reflect.deleteProperty(navigator, 'clipboard')
  })

  it('renders nothing without detail text', () => {
    const { rerender } = renderWithProviders(<SupportDetails detail={null} />)
    expect(screen.queryByTestId('support-details')).toBeNull()

    rerender(<SupportDetails detail="   " />)
    expect(screen.queryByTestId('support-details')).toBeNull()
  })

  it('keeps the raw text collapsed behind a localized summary', () => {
    renderWithProviders(<SupportDetails detail="policy.denied: egress budget exhausted" />)

    const details = screen.getByTestId('support-details')
    expect(details).not.toHaveAttribute('open')
    expect(screen.getByText(en.errors.supportDetails)).toBeInTheDocument()
    expect(screen.getByTestId('support-details-text')).toHaveTextContent('policy.denied: egress budget exhausted')
  })

  it('copies the exact text and resets the confirmation', async () => {
    vi.useFakeTimers()
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    renderWithProviders(<SupportDetails detail={'  chat.subprocess: consent_required\n'} />)

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: en.errors.copySupportDetails }))
    })

    expect(writeText).toHaveBeenCalledWith('chat.subprocess: consent_required')
    expect(screen.getByRole('button', { name: en.errors.supportDetailsCopied })).toBeInTheDocument()

    act(() => {
      vi.advanceTimersByTime(2000)
    })
    expect(screen.getByRole('button', { name: en.errors.copySupportDetails })).toBeInTheDocument()
  })

  it('reports when the panel opens', () => {
    const onToggle = vi.fn()
    renderWithProviders(<SupportDetails detail="storage.unavailable" onToggle={onToggle} />)

    const details = screen.getByTestId('support-details') as HTMLDetailsElement
    details.open = true
    fireEvent(details, new Event('toggle'))

    expect(onToggle).toHaveBeenCalledWith(true)
  })
})
