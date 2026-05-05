import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import SidePanel from '../SidePanel'

const defaultProps = {
  collapsed: false,
  width: 260,
  onResizeStart: vi.fn(),
  onResizeByKeyboard: vi.fn(),
}

describe('SidePanel', () => {
  it('has displayName', () => {
    expect(SidePanel.displayName).toBe('SidePanel')
  })

  it('returns null when collapsed', () => {
    const { container } = renderWithProviders(<SidePanel {...defaultProps} collapsed={true} />)
    expect(container.innerHTML).toBe('')
  })

  it('renders content when not collapsed', () => {
    renderWithProviders(<SidePanel {...defaultProps} />)
    expect(screen.getByRole('tree')).toBeInTheDocument()
  })

  it('resize handle has role="separator"', () => {
    renderWithProviders(<SidePanel {...defaultProps} />)
    const separator = screen.getByRole('separator')
    expect(separator).toBeInTheDocument()
    expect(separator).toHaveAttribute('aria-orientation', 'vertical')
  })

  it('resize handle reports current width', () => {
    renderWithProviders(<SidePanel {...defaultProps} width={300} />)
    const separator = screen.getByRole('separator')
    expect(separator).toHaveAttribute('aria-valuenow', '300')
  })

  it('renders the full monitor group tree when on /day (leaf route in monitor)', () => {
    // Previously /day was childless so the panel hid itself.  After the
    // category restructure, /day belongs to the monitor group — the panel
    // shows the entire monitor tree with Day highlighted as a leaf treeitem.
    renderWithProviders(<SidePanel {...defaultProps} />, {
      routerProps: { initialEntries: ['/day'] },
    })
    expect(screen.getByRole('tree')).toBeInTheDocument()
    // Day should be the selected treeitem (leaf in the tree).
    const selected = screen.getByRole('treeitem', { selected: true })
    expect(selected).toHaveTextContent(/day view/i)
  })

  it('renders the full data group tree when on /chat (leaf route in data)', () => {
    renderWithProviders(<SidePanel {...defaultProps} />, {
      routerProps: { initialEntries: ['/chat'] },
    })
    expect(screen.getByRole('tree')).toBeInTheDocument()
    const selected = screen.getByRole('treeitem', { selected: true })
    expect(selected).toHaveTextContent(/chat/i)
  })

  it('renders tree for /focus/score (nested route highlights the child)', () => {
    renderWithProviders(<SidePanel {...defaultProps} />, {
      routerProps: { initialEntries: ['/focus/score'] },
    })
    expect(screen.getByRole('tree')).toBeInTheDocument()
    const selected = screen.getByRole('treeitem', { selected: true })
    expect(selected).toHaveTextContent(/current score/i)
  })

  it('renders settings children when on a bottom route (legacy mode)', () => {
    // Bottom items (Settings, Privacy) do NOT belong to any group, so the
    // panel falls back to showing the current route's children only.
    renderWithProviders(<SidePanel {...defaultProps} />, {
      routerProps: { initialEntries: ['/settings/general'] },
    })
    expect(screen.getByRole('tree')).toBeInTheDocument()
    // General should be selected.
    const selected = screen.getByRole('treeitem', { selected: true })
    expect(selected).toHaveTextContent(/general/i)
  })

  it('renders collapse button only when onCollapse is provided', () => {
    const { rerender } = renderWithProviders(<SidePanel {...defaultProps} />, {
      routerProps: { initialEntries: ['/focus/score'] },
    })
    expect(screen.queryByTestId('sidepanel-collapse')).not.toBeInTheDocument()

    rerender(<SidePanel {...defaultProps} onCollapse={vi.fn()} />)
    expect(screen.getByTestId('sidepanel-collapse')).toBeInTheDocument()
  })

  it('clicking the collapse button invokes onCollapse', async () => {
    const user = userEvent.setup()
    const onCollapse = vi.fn()
    renderWithProviders(<SidePanel {...defaultProps} onCollapse={onCollapse} />, {
      routerProps: { initialEntries: ['/focus/score'] },
    })

    await user.click(screen.getByTestId('sidepanel-collapse'))
    expect(onCollapse).toHaveBeenCalledTimes(1)
  })
})
