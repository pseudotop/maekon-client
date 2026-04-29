import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import ShortcutsHelp from '../ShortcutsHelp'

describe('ShortcutsHelp', () => {
  it('has displayName', () => {
    expect(ShortcutsHelp.displayName).toBe('ShortcutsHelp')
  })

  it('renders dialog with aria-modal', () => {
    renderWithProviders(<ShortcutsHelp onClose={vi.fn()} />)
    const dialog = screen.getByRole('dialog')
    expect(dialog).toBeInTheDocument()
    expect(dialog).toHaveAttribute('aria-modal', 'true')
  })

  it('renders shortcut list', () => {
    renderWithProviders(<ShortcutsHelp onClose={vi.fn()} />)
    // Should display kbd elements for each shortcut
    const kbdElements = screen.getAllByText(/^[A-Z?]$|ESC|Enter|←|⌘|Ctrl/i)
    expect(kbdElements.length).toBeGreaterThan(0)
  })

  it('close button calls onClose', async () => {
    const user = userEvent.setup()
    const onClose = vi.fn()
    renderWithProviders(<ShortcutsHelp onClose={onClose} />)

    const closeBtn = screen.getByRole('button', { name: /close/i })
    await user.click(closeBtn)
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('has labelled title', () => {
    renderWithProviders(<ShortcutsHelp onClose={vi.fn()} />)
    const dialog = screen.getByRole('dialog')
    // Dialog auto-generates the labelledby ID via React useId().
    // Verify the attribute exists and points to an element in the document.
    const labelledById = dialog.getAttribute('aria-labelledby')
    expect(labelledById).toBeTruthy()
    if (labelledById) {
      expect(document.getElementById(labelledById)).toBeInTheDocument()
    }
  })
})
