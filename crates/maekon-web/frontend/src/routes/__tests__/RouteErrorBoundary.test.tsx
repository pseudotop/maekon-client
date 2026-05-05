import { act, fireEvent, render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// Hoisted mock — vi.mock is hoisted to the top of the file by Vitest, so any
// variable referenced in the factory must be hoisted as well via vi.hoisted.
const { reportToNativeMock } = vi.hoisted(() => ({
  reportToNativeMock: vi.fn(),
}))
vi.mock('../reportToNative', () => ({
  reportToNative: reportToNativeMock,
}))

import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { OutletContextError } from '../OutletContextError'
import { RouteErrorBoundary } from '../RouteErrorBoundary'
import { _resetRecoverySignalsForTest, notifyRouteRecovery } from '../recoverySignals'

interface ThrowerProps {
  shouldThrow: boolean
  error?: Error
}

function Thrower({ shouldThrow, error }: ThrowerProps) {
  if (shouldThrow) {
    throw error ?? new Error('Test crash')
  }
  return <div>Working</div>
}

function renderBoundary(children: ReactNode, route = '/test') {
  return renderWithProviders(<RouteErrorBoundary route={route}>{children}</RouteErrorBoundary>)
}

describe('RouteErrorBoundary', () => {
  beforeEach(() => {
    reportToNativeMock.mockReset()
    _resetRecoverySignalsForTest()
    // React error boundaries log the caught error to console.error — silence
    // it so the test output stays clean.
    vi.spyOn(console, 'error').mockImplementation(() => {})
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('renders children when no error is thrown', () => {
    renderBoundary(<Thrower shouldThrow={false} />)
    expect(screen.getByText('Working')).toBeInTheDocument()
    expect(reportToNativeMock).not.toHaveBeenCalled()
  })

  it('renders fallback UI when a child throws', () => {
    renderBoundary(<Thrower shouldThrow={true} />, '/focus')

    // Working content is gone
    expect(screen.queryByText('Working')).not.toBeInTheDocument()
    // Fallback alert is mounted
    expect(screen.getByRole('alert')).toBeInTheDocument()
  })

  it('reports caught errors to the native bridge with route metadata', () => {
    renderBoundary(<Thrower shouldThrow={true} error={new Error('boom')} />, '/automation')

    expect(reportToNativeMock).toHaveBeenCalledTimes(1)
    expect(reportToNativeMock).toHaveBeenCalledWith(
      expect.objectContaining({
        route: '/automation',
        message: 'boom',
        severity: 'error',
      }),
    )
  })

  it('classifies network errors as warning severity', () => {
    const networkError = new TypeError('Failed to fetch')
    renderBoundary(<Thrower shouldThrow={true} error={networkError} />, '/dashboard')

    expect(reportToNativeMock).toHaveBeenCalledTimes(1)
    expect(reportToNativeMock).toHaveBeenCalledWith(
      expect.objectContaining({
        severity: 'warning',
      }),
    )
  })

  it('classifies OutletContextError as error severity', () => {
    const ctxError = new OutletContextError('FocusRoute')
    renderBoundary(<Thrower shouldThrow={true} error={ctxError} />, '/focus')

    expect(reportToNativeMock).toHaveBeenCalledTimes(1)
    expect(reportToNativeMock).toHaveBeenCalledWith(
      expect.objectContaining({
        severity: 'error',
      }),
    )
  })

  it('renders the retry button when an error is caught', () => {
    renderBoundary(<Thrower shouldThrow={true} />, '/test')

    const buttons = screen.getAllByRole('button')
    // Fallback shows two buttons: retry + go home
    expect(buttons.length).toBeGreaterThanOrEqual(2)
  })

  it('resets via notifyRouteRecovery for the matching route', () => {
    const recoveryRoute = `/test-recovery-${Date.now()}-${Math.random()}`
    // Module-level state controls thrower behavior. After error caught,
    // we flip the flag, then notify the registry for this route.
    const state = { shouldThrow: true }
    const ControlledThrower = () => {
      if (state.shouldThrow) throw new Error('controlled crash')
      return <div>Recovered</div>
    }

    renderBoundary(<ControlledThrower />, recoveryRoute)
    // Fallback should be visible after the throw
    expect(screen.getByRole('alert')).toBeInTheDocument()

    // Now stop throwing and notify the registry for THIS route.
    // Wrap in act() so React flushes the resulting setResetKey state update.
    state.shouldThrow = false
    act(() => {
      notifyRouteRecovery(recoveryRoute)
    })

    // Boundary remounts and renders the now-working component
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.getByText('Recovered')).toBeInTheDocument()
  })

  it('ignores notifyRouteRecovery for a different route', () => {
    const myRoute = `/test-my-route-${Date.now()}`
    const otherRoute = `/test-other-route-${Date.now()}`
    renderBoundary(<Thrower shouldThrow={true} />, myRoute)
    expect(screen.getByRole('alert')).toBeInTheDocument()

    // Notify for a DIFFERENT route — boundary should NOT reset
    act(() => {
      notifyRouteRecovery(otherRoute)
    })

    // Fallback still visible
    expect(screen.getByRole('alert')).toBeInTheDocument()
  })

  it('escalates auto-recovery to critical after 3+ signals within window', () => {
    // The auto-recovery path (via notifyRouteRecovery) should also count
    // towards the escalation threshold. After 3 auto-recoveries on the same
    // crashing component, severity escalates to critical and the local
    // remount is skipped (Rust will trigger full-reload).
    const route = `/test-auto-escalation-${Date.now()}`
    renderBoundary(<Thrower shouldThrow={true} />, route)
    expect(screen.getByRole('alert')).toBeInTheDocument()
    // Initial caught error
    expect(reportToNativeMock).toHaveBeenLastCalledWith(expect.objectContaining({ severity: 'error' }))

    // Trigger 3 auto-recoveries in rapid succession (synchronous within
    // the 60s window). On the 3rd, escalation must fire.
    act(() => {
      notifyRouteRecovery(route)
      notifyRouteRecovery(route)
      notifyRouteRecovery(route)
    })

    // The escalation path reports critical without local remount
    const criticalCall = reportToNativeMock.mock.calls.find(([payload]) => payload?.severity === 'critical')
    expect(criticalCall).toBeDefined()
    expect(criticalCall?.[0]).toMatchObject({ route })
  })

  it('clears resetTracker entry on unmount', () => {
    const route = `/test-cleanup-${Date.now()}`
    const { unmount } = renderBoundary(<Thrower shouldThrow={true} />, route)
    // Trigger one tracked reset to populate the entry
    const buttons = screen.getAllByRole('button')
    fireEvent.click(buttons[0])

    unmount()

    // After unmount, a fresh visit must start with a clean window.
    // Render a new boundary at the same route — the prior count must NOT carry over.
    reportToNativeMock.mockClear()
    renderBoundary(<Thrower shouldThrow={true} />, route)
    // Only one error should have been reported (the new mount), not an escalation
    expect(reportToNativeMock).toHaveBeenCalledTimes(1)
    expect(reportToNativeMock).toHaveBeenLastCalledWith(expect.objectContaining({ severity: 'error' }))
  })

  it('escalates to critical severity after repeated retries within the window', () => {
    // Use a unique route per test to avoid leaking module-level reset state
    // between cases. Three retries within 60s on the same route → critical.
    const escalationRoute = `/test-escalation-${Date.now()}-${Math.random()}`
    renderBoundary(<Thrower shouldThrow={true} />, escalationRoute)

    // Initial caught error → severity "error"
    expect(reportToNativeMock).toHaveBeenLastCalledWith(expect.objectContaining({ severity: 'error' }))

    // Each click triggers handleRetry → trackReset → eventually critical.
    // After each click the fallback re-renders, so we re-query the button.
    for (let i = 0; i < 3; i++) {
      const buttons = screen.getAllByRole('button')
      // Retry is the primary button rendered first in RouteErrorFallback.
      fireEvent.click(buttons[0])
    }

    // The third click hits the escalation threshold and produces a
    // critical-severity report.
    const criticalCall = reportToNativeMock.mock.calls.find(([payload]) => payload?.severity === 'critical')
    expect(criticalCall).toBeDefined()
    expect(criticalCall?.[0]).toMatchObject({ route: escalationRoute })
  })

  it('shows recovering state after critical escalation', () => {
    // IMPORTANT-3 regression: verify the isRecovering UI path renders when
    // escalation fires. Without this test, a regression that breaks the
    // prop wiring or the fallback early-return would ship silently.
    const route = `/test-recovering-${Date.now()}-${Math.random()}`
    renderBoundary(<Thrower shouldThrow={true} />, route)

    // Click retry 3 times to hit the escalation threshold
    for (let i = 0; i < 3; i++) {
      const buttons = screen.getAllByRole('button')
      fireEvent.click(buttons[0])
    }

    // The recovering state has NO "Try Again" button — it has a spinner
    // and the i18n-translated title. Using role='alert' is still present
    // but the primary button should be gone.
    const recoveringAlert = screen.getByRole('alert')
    expect(recoveringAlert).toBeInTheDocument()
    // No buttons in recovering state (Try Again + Go Home are both hidden)
    expect(screen.queryAllByRole('button')).toHaveLength(0)
  })

  it('schedules a safety-net reload on critical escalation and cancels on unmount (CRITICAL-1)', () => {
    // The safety-net setTimeout must be cleared on unmount — otherwise
    // an uncleared timer fires window.location.reload() after the user
    // has navigated away, destroying unrelated work.
    vi.useFakeTimers()
    const originalLocation = window.location
    const reloadMock = vi.fn()
    Object.defineProperty(window, 'location', {
      configurable: true,
      value: { ...originalLocation, reload: reloadMock },
    })

    const route = `/test-safety-net-${Date.now()}-${Math.random()}`
    const { unmount } = renderBoundary(<Thrower shouldThrow={true} />, route)

    // Trigger escalation via 3 retries
    for (let i = 0; i < 3; i++) {
      const buttons = screen.getAllByRole('button')
      fireEvent.click(buttons[0])
    }

    // Safety-net is armed. Unmount BEFORE the 5s timer fires.
    unmount()

    // Advance past the 5s window
    vi.advanceTimersByTime(6_000)

    // reload was NOT called because the timer was cleared on unmount
    expect(reloadMock).not.toHaveBeenCalled()

    // Cleanup
    Object.defineProperty(window, 'location', {
      configurable: true,
      value: originalLocation,
    })
    vi.useRealTimers()
  })

  it('safety-net reload fires after 5s if not cancelled', () => {
    vi.useFakeTimers()
    const originalLocation = window.location
    const reloadMock = vi.fn()
    Object.defineProperty(window, 'location', {
      configurable: true,
      value: { ...originalLocation, reload: reloadMock },
    })

    const route = `/test-safety-fires-${Date.now()}-${Math.random()}`
    renderBoundary(<Thrower shouldThrow={true} />, route)

    for (let i = 0; i < 3; i++) {
      const buttons = screen.getAllByRole('button')
      fireEvent.click(buttons[0])
    }

    // Safety-net is armed. Without intervention, after 5s it fires reload.
    vi.advanceTimersByTime(6_000)
    expect(reloadMock).toHaveBeenCalledTimes(1)

    Object.defineProperty(window, 'location', {
      configurable: true,
      value: originalLocation,
    })
    vi.useRealTimers()
  })

  it('resets the error state when pathname changes (IMPORTANT-2 / IA I3)', () => {
    // When the user is stuck on an error at /focus/score and clicks a
    // sibling route (/focus/sessions), the parent boundary stays mounted
    // but must reset its error state so the new sub-route can render.
    const BadChild = () => {
      throw new Error('boom')
    }
    const GoodChild = () => <div>Good content</div>

    function Harness() {
      const navigate = useNavigate()
      return (
        <div>
          <button type="button" onClick={() => navigate('/focus/sessions')}>
            go-sessions
          </button>
          <Routes>
            <Route
              path="/focus/score"
              element={
                <RouteErrorBoundary route="/focus">
                  <BadChild />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/focus/sessions"
              element={
                <RouteErrorBoundary route="/focus">
                  <GoodChild />
                </RouteErrorBoundary>
              }
            />
          </Routes>
        </div>
      )
    }

    render(
      <MemoryRouter initialEntries={['/focus/score']}>
        <Harness />
      </MemoryRouter>,
    )

    // Error shown at /focus/score
    expect(screen.getByRole('alert')).toBeInTheDocument()

    // Navigate to sibling — the boundary should reset
    fireEvent.click(screen.getByText('go-sessions'))

    // New sub-route renders without the error fallback
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.getByText('Good content')).toBeInTheDocument()
  })

  it('does not double-escalate when another recovery signal arrives while already recovering (IMPORTANT-1)', () => {
    // Once isRecovering is true, subsequent notifyRouteRecovery calls must
    // be ignored — otherwise multiple safety-net timers get scheduled.
    const route = `/test-no-double-${Date.now()}-${Math.random()}`
    renderBoundary(<Thrower shouldThrow={true} />, route)

    // Trigger escalation via 3 manual retries
    for (let i = 0; i < 3; i++) {
      const buttons = screen.getAllByRole('button')
      fireEvent.click(buttons[0])
    }

    // Clear the mock after the escalation
    const criticalCallsBefore = reportToNativeMock.mock.calls.filter(
      ([payload]) => payload?.severity === 'critical',
    ).length
    reportToNativeMock.mockClear()

    // Fire another recovery notification — it should be ignored
    act(() => {
      notifyRouteRecovery(route)
    })

    // No NEW critical reports fired (the ref-guard blocks it)
    const criticalCallsAfter = reportToNativeMock.mock.calls.filter(
      ([payload]) => payload?.severity === 'critical',
    ).length
    expect(criticalCallsBefore).toBeGreaterThanOrEqual(1)
    expect(criticalCallsAfter).toBe(0)
  })
})
