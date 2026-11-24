import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import { navGroups, routeTree } from '../../../routes'
import ActivityBar from '../ActivityBar'

const defaultProps = {
  onToggleSidebar: vi.fn(),
  sidebarCollapsed: false,
}

// Activity bar shape (#13736): one main group button + every bottom item
// (Settings, Privacy) rendered as its own direct icon.
const expectedBottomItems = routeTree.filter((r) => r.bottom && r.icon).length
const expectedNavButtonCount = navGroups.length + expectedBottomItems

describe('ActivityBar', () => {
  it('has displayName', () => {
    expect(ActivityBar.displayName).toBe('ActivityBar')
  })

  it('renders navigation landmark', () => {
    renderWithProviders(<ActivityBar {...defaultProps} />)
    expect(screen.getByRole('navigation')).toBeInTheDocument()
  })

  it('renders exactly one button per nav group + one per bottom item', () => {
    renderWithProviders(<ActivityBar {...defaultProps} />)
    const buttons = screen.getAllByRole('button')
    expect(buttons).toHaveLength(expectedNavButtonCount)
  })

  it('renders a testid for every nav group', () => {
    renderWithProviders(<ActivityBar {...defaultProps} />)
    for (const group of navGroups) {
      expect(screen.getByTestId(`nav-group-${group.id}`)).toBeInTheDocument()
    }
  })

  it('keeps only Settings and Privacy as direct bottom icons (#13736)', () => {
    renderWithProviders(<ActivityBar {...defaultProps} />)
    expect(screen.getByTestId('nav-settings')).toBeInTheDocument()
    expect(screen.getByTestId('nav-privacy')).toBeInTheDocument()
    // Support moved one level back into the System bundle of the main tree.
    expect(screen.queryByTestId('nav-support')).not.toBeInTheDocument()
    expect(expectedBottomItems).toBe(2)
  })

  it('group icon is active when pathname matches a route in that group', () => {
    renderWithProviders(<ActivityBar {...defaultProps} />, {
      routerProps: { initialEntries: ['/automation/policies'] },
    })
    const activeButton = screen.getByRole('button', { current: 'page' })
    expect(activeButton).toHaveAttribute('data-testid', 'nav-group-main')
  })

  it('clicking an inactive group navigates to its default path', async () => {
    const user = userEvent.setup()
    const onToggleSidebar = vi.fn()
    renderWithProviders(<ActivityBar onToggleSidebar={onToggleSidebar} sidebarCollapsed={true} />, {
      routerProps: { initialEntries: ['/settings/general'] },
    })

    // Clicking the main group from Settings → group changes + sidebar toggles
    // (because the sidebar was collapsed).
    await user.click(screen.getByTestId('nav-group-main'))
    expect(onToggleSidebar).toHaveBeenCalled()
  })

  it('clicking an inactive group while sidebar is open navigates without toggling', async () => {
    // VS Code pattern: switching to a different group should keep the sidebar
    // in its current state (open). Only clicking the ACTIVE group toggles.
    const user = userEvent.setup()
    const onToggleSidebar = vi.fn()
    renderWithProviders(<ActivityBar onToggleSidebar={onToggleSidebar} sidebarCollapsed={false} />, {
      routerProps: { initialEntries: ['/privacy/data'] },
    })

    // From Privacy, click the main group while the sidebar is already expanded.
    await user.click(screen.getByTestId('nav-group-main'))
    expect(onToggleSidebar).not.toHaveBeenCalled()
  })

  it('clicking the active group toggles the sidebar (VS Code-style)', async () => {
    const user = userEvent.setup()
    const onToggleSidebar = vi.fn()
    renderWithProviders(<ActivityBar onToggleSidebar={onToggleSidebar} sidebarCollapsed={false} />, {
      routerProps: { initialEntries: ['/reports/activity'] },
    })

    // /reports is in the main group — clicking it while the sidebar is open
    // should collapse it without navigating elsewhere.
    await user.click(screen.getByTestId('nav-group-main'))
    expect(onToggleSidebar).toHaveBeenCalledTimes(1)
  })

  it('clicking the active bottom item toggles the sidebar', async () => {
    const user = userEvent.setup()
    const onToggleSidebar = vi.fn()
    renderWithProviders(<ActivityBar onToggleSidebar={onToggleSidebar} sidebarCollapsed={false} />, {
      routerProps: { initialEntries: ['/settings/general'] },
    })

    await user.click(screen.getByTestId('nav-settings'))
    expect(onToggleSidebar).toHaveBeenCalledTimes(1)
  })

  it('each button has an aria-label', () => {
    renderWithProviders(<ActivityBar {...defaultProps} />)
    const buttons = screen.getAllByRole('button')
    buttons.forEach((btn) => {
      expect(btn).toHaveAttribute('aria-label')
    })
  })
})
