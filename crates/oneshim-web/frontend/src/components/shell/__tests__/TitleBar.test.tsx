import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import TitleBar from '../TitleBar'

describe('TitleBar', () => {
  it('has displayName', () => {
    expect(TitleBar.displayName).toBe('TitleBar')
  })

  it('renders current page title from route', () => {
    renderWithProviders(<TitleBar onSearchOpen={vi.fn()} />)
    // Default route "/" maps to nav.dashboard → "Dashboard"
    expect(screen.getByText('Dashboard')).toBeInTheDocument()
  })

  it('search button has aria-label', () => {
    renderWithProviders(<TitleBar onSearchOpen={vi.fn()} />)
    const btn = screen.getByRole('button', { name: /search/i })
    expect(btn).toBeInTheDocument()
  })

  it('calls onSearchOpen when search button clicked', async () => {
    const user = userEvent.setup()
    const onSearchOpen = vi.fn()
    renderWithProviders(<TitleBar onSearchOpen={onSearchOpen} />)

    await user.click(screen.getByRole('button', { name: /search/i }))
    expect(onSearchOpen).toHaveBeenCalledOnce()
  })

  it('shows parent label only when at default child sub-route', () => {
    // /focus/score is the defaultChild — expect "Focus" (no " › Score")
    renderWithProviders(<TitleBar onSearchOpen={vi.fn()} />, {
      routerProps: { initialEntries: ['/focus/score'] },
    })
    expect(screen.getByText('Focus')).toBeInTheDocument()
    expect(screen.queryByText(/›/)).not.toBeInTheDocument()
  })

  it('shows "Parent › Child" for non-default sub-route (C2 regression)', () => {
    // /focus/sessions is NOT the defaultChild — expect "Focus › Focus Sessions"
    renderWithProviders(<TitleBar onSearchOpen={vi.fn()} />, {
      routerProps: { initialEntries: ['/focus/sessions'] },
    })
    expect(screen.getByText(/Focus.*›/)).toBeInTheDocument()
  })

  it('shows correct title for previously-missing routes (/day, /audit, /chat)', () => {
    // The old hardcoded pageTitleKeys map missed these routes and fell back
    // to "Dashboard". Verify the routeTree-derived title now works.
    const { unmount: u1 } = renderWithProviders(<TitleBar onSearchOpen={vi.fn()} />, {
      routerProps: { initialEntries: ['/day'] },
    })
    expect(screen.getByText('Day View')).toBeInTheDocument()
    u1()

    const { unmount: u2 } = renderWithProviders(<TitleBar onSearchOpen={vi.fn()} />, {
      routerProps: { initialEntries: ['/audit'] },
    })
    expect(screen.getByText(/Audit/)).toBeInTheDocument()
    u2()

    renderWithProviders(<TitleBar onSearchOpen={vi.fn()} />, {
      routerProps: { initialEntries: ['/chat'] },
    })
    expect(screen.getByText('Chat')).toBeInTheDocument()
  })
})
