import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { AppMemoryRouter } from '../../router/future'

type EventCallback = (event: { payload?: unknown }) => void

type RenderBridgeHarnessOptions = {
  expectedListenCalls?: number
  listenImpl?: (eventName: string, callback: EventCallback) => Promise<() => void>
}

// 6 global listen calls (navigate, tray-approve-update, tray-defer-update,
// oauth-reauth-required, navigate:chat, integration-proactive-prompt) + 1
// webview-scoped listen for frontend-recovery. The previous
// `tray-toggle-automation` and `automation:quick-access` events were folded
// into the single `navigate` event after the tray IPC unification — tray.rs
// now emits plain `navigate` with a deep-link path for those menu items.
async function renderBridgeHarness({ expectedListenCalls = 6, listenImpl }: RenderBridgeHarnessOptions = {}) {
  const listeners = new Map<string, EventCallback>()
  const unlistenCallbacks: Array<ReturnType<typeof vi.fn>> = []
  const defaultListenImpl = async (eventName: string, callback: EventCallback) => {
    listeners.set(eventName, callback)
    const unlisten = vi.fn(() => {
      listeners.delete(eventName)
    })
    unlistenCallbacks.push(unlisten)
    return unlisten
  }
  const listen = vi.fn(listenImpl ?? defaultListenImpl)
  // Webview-scoped listener (used for frontend-recovery so emit_to filters
  // delivery on the Rust side). Same shape as global listen for the test.
  const webviewListen = vi.fn(listenImpl ?? defaultListenImpl)

  vi.doMock('../../utils/platform', () => ({
    IS_TAURI: true,
  }))
  vi.doMock('@tauri-apps/api/event', () => ({
    listen,
  }))
  vi.doMock('@tauri-apps/api/webview', () => ({
    getCurrentWebview: () => ({
      listen: webviewListen,
    }),
  }))

  const { useTauriEventBridge } = await import('../useTauriEventBridge')
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  })
  const invalidateQueries = vi.spyOn(queryClient, 'invalidateQueries')

  function LocationProbe() {
    useTauriEventBridge()
    const location = useLocation()
    return <div data-testid="path">{location.pathname}</div>
  }

  function wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <AppMemoryRouter initialEntries={['/']}>{children}</AppMemoryRouter>
      </QueryClientProvider>
    )
  }

  const rendered = render(<LocationProbe />, { wrapper })
  await waitFor(() => expect(listen).toHaveBeenCalledTimes(expectedListenCalls))

  return {
    ...rendered,
    invalidateQueries,
    listeners,
    listen,
    webviewListen,
    unlistenCallbacks,
  }
}

describe('useTauriEventBridge', () => {
  afterEach(() => {
    vi.resetModules()
    vi.clearAllMocks()
  })

  it('routes navigate payloads and cleans up listeners on unmount', async () => {
    const { listeners, unlistenCallbacks, unmount } = await renderBridgeHarness()

    act(() => {
      listeners.get('navigate')?.({ payload: '/settings' })
    })

    expect(screen.getByTestId('path')).toHaveTextContent('/settings')

    unmount()

    expect(unlistenCallbacks).toHaveLength(7)
    unlistenCallbacks.forEach((unlisten) => {
      expect(unlisten).toHaveBeenCalledTimes(1)
    })
  })

  it('routes tray update events to /updates and refreshes update-status', async () => {
    // Post tray-IPC unification, the "AI Automation Preferences" and
    // "Automation Page" menu items emit plain `navigate` events, so only the
    // update-approve / update-defer events still need their own listeners
    // (they pair navigation with an `update-status` refetch).
    const { invalidateQueries, listeners } = await renderBridgeHarness()

    act(() => {
      listeners.get('tray-approve-update')?.({})
    })

    expect(screen.getByTestId('path')).toHaveTextContent('/updates')
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['update-status'] })

    act(() => {
      listeners.get('tray-defer-update')?.({})
    })

    expect(screen.getByTestId('path')).toHaveTextContent('/updates')
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['update-status'] })
  })

  it('cleans up earlier listeners when a later registration fails', async () => {
    let callCount = 0
    const listenerMap = new Map<string, EventCallback>()
    const unlistenCallbacks: Array<ReturnType<typeof vi.fn>> = []

    await renderBridgeHarness({
      expectedListenCalls: 3,
      listenImpl: async (eventName, callback) => {
        callCount += 1
        if (callCount === 3) {
          throw new Error('listen failed')
        }

        listenerMap.set(eventName, callback)
        const unlisten = vi.fn(() => {
          listenerMap.delete(eventName)
        })
        unlistenCallbacks.push(unlisten)
        return unlisten
      },
    })

    await waitFor(() => expect(unlistenCallbacks).toHaveLength(2))
    expect(listenerMap.size).toBe(0)
    unlistenCallbacks.forEach((unlisten) => {
      expect(unlisten).toHaveBeenCalledTimes(1)
    })
  })

  it('drains earlier listeners when unmount fires during registration (NC-NEW-3 + IMPORTANT-3)', async () => {
    // Simulates: unmount() called while one of the listen() calls is still
    // pending. The pre-fix code would early-return after the await completed
    // (because `disposed` was true), leaking the already-pushed callbacks.
    // The fix throws DisposedDuringRegistration which the catch block drains.
    //
    // IMPORTANT-3 regression: also verify that console.warn is NOT called
    // for the disposal path (the U4 fix at useTauriEventBridge.ts uses
    // `instanceof DisposedDuringRegistration` to suppress the noisy warning).
    // Without this assertion, a regression to `e.constructor.name !== ...`
    // would silently re-emit warnings in production builds.
    const consoleWarn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const listenerMap = new Map<string, EventCallback>()
    const allUnlistens: Array<ReturnType<typeof vi.fn>> = []
    let pendingResolve: ((value: () => void) => void) | null = null

    const harness = renderBridgeHarness({
      expectedListenCalls: 3,
      listenImpl: async (eventName, callback) => {
        listenerMap.set(eventName, callback)
        const unlisten = vi.fn(() => {
          listenerMap.delete(eventName)
        })
        allUnlistens.push(unlisten)
        // After 2 successful calls, hold the 3rd in flight
        if (allUnlistens.length === 3) {
          await new Promise<() => void>((resolve) => {
            pendingResolve = resolve
          })
        }
        return unlisten
      },
    })

    // Wait until 2 listeners registered + 3rd is pending
    await waitFor(() => expect(allUnlistens).toHaveLength(3))

    // Unmount BEFORE the 3rd listen resolves — simulates fast component
    // unmount during registration. The harness already rendered, so we
    // need to manually resolve from outside.
    const rendered = await harness
    rendered.unmount()

    // Now resolve the pending listen — DisposedDuringRegistration should fire
    pendingResolve?.(() => {})

    // Wait for the catch to drain
    await waitFor(() => {
      expect(listenerMap.size).toBe(0)
    })
    // All earlier registrations were unlistened
    for (const unlisten of allUnlistens) {
      expect(unlisten).toHaveBeenCalled()
    }
    // Critical: warn must NOT have been called for the disposal path.
    // Catches a regression where the catch block goes back to using
    // `e.constructor.name`, which mangles under esbuild minification.
    expect(consoleWarn).not.toHaveBeenCalledWith(
      expect.stringContaining('listener registration failed'),
      expect.anything(),
    )
    consoleWarn.mockRestore()
  })

  it('registers frontend-recovery via webview-scoped listener (NC-3 + NC-NEW-2)', async () => {
    vi.doMock('../../routes/recoverySignals', () => ({
      notifyRouteRecovery: vi.fn(),
    }))
    const { webviewListen } = await renderBridgeHarness()
    expect(webviewListen).toHaveBeenCalledTimes(1)
    expect(webviewListen).toHaveBeenCalledWith('frontend-recovery', expect.any(Function))
  })

  it('falls back to global listen when webview API import fails (NC-NEW-1 + U2)', async () => {
    // Simulates a missing or broken @tauri-apps/api/webview module.
    // The bridge should still register all 7 listeners — frontend-recovery
    // falls back to the global listen() instead of the scoped variant.
    const listeners = new Map<string, EventCallback>()
    const unlistenCallbacks: Array<ReturnType<typeof vi.fn>> = []
    const listen = vi.fn(async (eventName: string, callback: EventCallback) => {
      listeners.set(eventName, callback)
      const unlisten = vi.fn(() => {
        listeners.delete(eventName)
      })
      unlistenCallbacks.push(unlisten)
      return unlisten
    })

    vi.doMock('../../utils/platform', () => ({ IS_TAURI: true }))
    vi.doMock('@tauri-apps/api/event', () => ({ listen }))
    // Webview module throws on import — simulates missing/broken module
    vi.doMock('@tauri-apps/api/webview', () => {
      throw new Error('webview module unavailable')
    })
    vi.doMock('../../routes/recoverySignals', () => ({ notifyRouteRecovery: vi.fn() }))

    // Spy on console.warn to verify the fallback message
    const consoleWarn = vi.spyOn(console, 'warn').mockImplementation(() => {})

    const { useTauriEventBridge } = await import('../useTauriEventBridge')
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })

    function Probe() {
      useTauriEventBridge()
      return null
    }
    function wrapper({ children }: { children: ReactNode }) {
      return (
        <QueryClientProvider client={queryClient}>
          <AppMemoryRouter initialEntries={['/']}>{children}</AppMemoryRouter>
        </QueryClientProvider>
      )
    }

    render(<Probe />, { wrapper })

    // All 7 listeners (6 normal + 1 fallback frontend-recovery) registered via global listen
    await waitFor(() => expect(listen).toHaveBeenCalledTimes(7))
    expect(listeners.has('frontend-recovery')).toBe(true)
    // Fallback warning was logged
    expect(consoleWarn).toHaveBeenCalledWith(expect.stringContaining('webview API unavailable'), expect.any(Error))

    consoleWarn.mockRestore()
  })

  it('calls notifyRouteRecovery on frontend-recovery reset-route signal', async () => {
    // Mock the recoverySignals module BEFORE importing useTauriEventBridge
    const notifySpy = vi.fn()
    vi.doMock('../../routes/recoverySignals', () => ({
      notifyRouteRecovery: notifySpy,
    }))

    const { listeners } = await renderBridgeHarness()

    act(() => {
      listeners.get('frontend-recovery')?.({
        payload: {
          strategy: 'reset-route',
          route: '/focus',
          reason: 'render crash',
        },
      })
    })

    expect(notifySpy).toHaveBeenCalledWith('/focus')
  })

  it('does NOT globally invalidate queries on reset-route (boundary handles it)', async () => {
    vi.doMock('../../routes/recoverySignals', () => ({
      notifyRouteRecovery: vi.fn(),
    }))

    const { invalidateQueries, listeners } = await renderBridgeHarness()

    act(() => {
      listeners.get('frontend-recovery')?.({
        payload: {
          strategy: 'reset-route',
          route: '/focus',
          reason: 'render crash',
        },
      })
    })

    // Bridge should NOT trigger a global invalidate — that's the boundary's job.
    // Earlier impl invalidated here AND in the boundary (IA-3 double invalidation).
    expect(invalidateQueries).not.toHaveBeenCalled()
  })

  it('triggers full reload on frontend-recovery full-reload signal', async () => {
    vi.doMock('../../routes/recoverySignals', () => ({
      notifyRouteRecovery: vi.fn(),
    }))
    const { listeners } = await renderBridgeHarness()

    // jsdom does not implement window.location.reload — stub it
    const reloadMock = vi.fn()
    const originalLocation = window.location
    Object.defineProperty(window, 'location', {
      configurable: true,
      writable: true,
      value: { ...originalLocation, reload: reloadMock },
    })

    act(() => {
      listeners.get('frontend-recovery')?.({
        payload: {
          strategy: 'full-reload',
          route: '/focus',
          reason: 'critical escalation',
        },
      })
    })

    expect(reloadMock).toHaveBeenCalledTimes(1)
    // The browser fires `beforeunload` automatically on location.reload();
    // SettingsFormProvider listens for that to guard unsaved data.
    Object.defineProperty(window, 'location', {
      configurable: true,
      writable: true,
      value: originalLocation,
    })
  })

  it('ignores frontend-recovery payloads that fail the type guard', async () => {
    const notifySpy = vi.fn()
    vi.doMock('../../routes/recoverySignals', () => ({
      notifyRouteRecovery: notifySpy,
    }))

    const { invalidateQueries, listeners } = await renderBridgeHarness()

    act(() => {
      // Missing required fields
      listeners.get('frontend-recovery')?.({ payload: { strategy: 42 } })
    })

    expect(invalidateQueries).not.toHaveBeenCalled()
    expect(notifySpy).not.toHaveBeenCalled()
  })

  it('shows a toast for inbound integration prompts', async () => {
    const addToast = vi.fn()
    vi.doMock('../useToast', () => ({
      addToast,
    }))

    const { listeners } = await renderBridgeHarness()

    act(() => {
      listeners.get('integration-proactive-prompt')?.({
        payload: {
          title: 'Review teammate note',
          body: 'A follow-up prompt is waiting in the inbox',
        },
      })
    })

    expect(addToast).toHaveBeenCalledWith(
      'info',
      'Review teammate note: A follow-up prompt is waiting in the inbox',
      10000,
    )
  })
})
