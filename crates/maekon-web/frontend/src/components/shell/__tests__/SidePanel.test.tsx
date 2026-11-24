import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useLocation } from 'react-router-dom'
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

  it('renders the main tree when on /day (a route inside the More bundle)', () => {
    // Previously /day was childless so the panel hid itself.  /day belongs
    // to the main group, so the panel shows the main tree with Day revealed
    // inside its bundle and highlighted as a leaf treeitem.
    renderWithProviders(<SidePanel {...defaultProps} />, {
      routerProps: { initialEntries: ['/day'] },
    })
    expect(screen.getByRole('tree')).toBeInTheDocument()
    // Day should be the selected treeitem (leaf in the tree).
    const selected = screen.getByRole('treeitem', { selected: true })
    expect(selected).toHaveTextContent(/day view/i)
  })

  it('renders the main tree when on /chat (a front leaf route)', () => {
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

function LocationProbe() {
  return <output data-testid="location">{useLocation().pathname}</output>
}

function firstLevelItems() {
  return Array.from(screen.getByRole('tree').querySelectorAll<HTMLElement>('[role="treeitem"][aria-level="1"]'))
}

// #13736: six front destinations, then bundles that start folded.
describe('SidePanel main tree', () => {
  it('shows six front destinations and three folded bundles by default', () => {
    renderWithProviders(<SidePanel {...defaultProps} />, { routerProps: { initialEntries: ['/overview'] } })
    expect(firstLevelItems().map((item) => item.textContent)).toEqual([
      'Dashboard',
      'Timeline',
      'Search',
      'Focus',
      'Reports',
      'Chat',
      'More',
      'Tools',
      'System',
    ])
    for (const bundle of ['More', 'Tools', 'System']) {
      expect(screen.getByRole('treeitem', { name: bundle })).toHaveAttribute('aria-expanded', 'false')
    }
    // Only the path to the current page is open.
    expect(screen.getByRole('treeitem', { name: 'Dashboard' })).toHaveAttribute('aria-expanded', 'true')
    expect(screen.getByRole('treeitem', { name: 'Timeline' })).toHaveAttribute('aria-expanded', 'false')
  })

  it('opens the bundle that holds the current page', () => {
    renderWithProviders(<SidePanel {...defaultProps} />, {
      routerProps: { initialEntries: ['/automation/history'] },
    })
    expect(screen.getByRole('treeitem', { name: 'Tools' })).toHaveAttribute('aria-expanded', 'true')
    expect(screen.getByRole('treeitem', { name: 'More' })).toHaveAttribute('aria-expanded', 'false')
    expect(screen.getByRole('treeitem', { selected: true })).toHaveTextContent('Execution History')
  })

  it('unfolds a bundle on click without navigating away', async () => {
    const user = userEvent.setup()
    renderWithProviders(
      <>
        <SidePanel {...defaultProps} />
        <LocationProbe />
      </>,
      { routerProps: { initialEntries: ['/overview'] } },
    )
    const system = screen.getByRole('treeitem', { name: 'System' })
    await user.click(system)

    expect(system).toHaveAttribute('aria-expanded', 'true')
    const group = system.parentElement?.querySelector<HTMLElement>('[role="group"]')
    expect(group).not.toBeNull()
    expect(within(group as HTMLElement).getByRole('treeitem', { name: 'Updates' })).toBeInTheDocument()
    expect(within(group as HTMLElement).getByRole('treeitem', { name: 'Support' })).toBeInTheDocument()
    expect(screen.getByTestId('location')).toHaveTextContent('/overview')
  })

  it('navigates to a destination inside a bundle', async () => {
    const user = userEvent.setup()
    renderWithProviders(
      <>
        <SidePanel {...defaultProps} />
        <LocationProbe />
      </>,
      { routerProps: { initialEntries: ['/overview'] } },
    )
    await user.click(screen.getByRole('treeitem', { name: 'More' }))
    await user.click(screen.getByRole('treeitem', { name: 'Week View' }))
    expect(screen.getByTestId('location')).toHaveTextContent('/week')
  })
})
