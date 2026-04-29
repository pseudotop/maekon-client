import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from 'react-router-dom'
import { notifyRouteRecovery } from '../routes/recoverySignals'
import { IS_TAURI } from '../utils/platform'
import { addToast } from './useToast'

type TauriEventPayload = {
  payload?: unknown
}

type IntegrationPromptPayload = {
  title?: unknown
  body?: unknown
}

function isRoutePath(payload: unknown): payload is string {
  return typeof payload === 'string' && payload.startsWith('/')
}

function isIntegrationPromptPayload(payload: unknown): payload is IntegrationPromptPayload {
  return typeof payload === 'object' && payload !== null
}

function isRecoveryPayload(payload: unknown): payload is { strategy: string; route: string; reason: string } {
  return (
    typeof payload === 'object' &&
    payload !== null &&
    'strategy' in payload &&
    'route' in payload &&
    typeof (payload as Record<string, unknown>).strategy === 'string' &&
    typeof (payload as Record<string, unknown>).route === 'string'
  )
}

function isChatPayload(payload: unknown): payload is { sessionId?: string } {
  if (typeof payload !== 'object' || payload === null) return false
  const obj = payload as Record<string, unknown>
  // sessionId is optional, but if present must be a string
  return !('sessionId' in obj) || typeof obj.sessionId === 'string'
}

/**
 * Sentinel error thrown when the listener-registration loop notices that the
 * effect has been disposed mid-await. The catch block uses `instanceof` to
 * distinguish this from a real registration failure.
 *
 * Module-scoped (not nested inside the effect) so that minifiers like esbuild
 * keep the prototype chain intact even when the class identifier is renamed.
 * The earlier `e.constructor.name` check broke under Vite's default minifier
 * because the local class name was mangled to a single letter.
 */
class DisposedDuringRegistration extends Error {}

export function useTauriEventBridge() {
  const navigate = useNavigate()
  const location = useLocation()
  const queryClient = useQueryClient()
  const { t } = useTranslation()
  const navigateRef = useRef(navigate)
  const pathnameRef = useRef(location.pathname)
  const queryClientRef = useRef(queryClient)
  const tRef = useRef(t)

  navigateRef.current = navigate
  pathnameRef.current = location.pathname
  queryClientRef.current = queryClient
  tRef.current = t

  useEffect(() => {
    if (!IS_TAURI) {
      return
    }

    let disposed = false
    let unlistenCallbacks: Array<() => void> = []

    const navigateTo = (path: string) => {
      if (pathnameRef.current === path) {
        return
      }
      navigateRef.current(path)
    }

    const refreshUpdateStatus = () => {
      void queryClientRef.current.invalidateQueries({ queryKey: ['update-status'] })
    }

    const registerListeners = async () => {
      let pendingUnlistenCallbacks: Array<() => void> = []

      try {
        const { listen } = await import('@tauri-apps/api/event')

        // For events that must NOT leak to other webviews (overlay /
        // tracking-panel), use the current-webview-scoped listener instead
        // of the global one. The Rust side then uses emit_to with the
        // matching label to actually filter the delivery.
        //
        // The webview import is in its own try so a failure here (version
        // skew, missing module) does not nuke the unrelated global listeners.
        // If unavailable, we fall back to the global `listen` for the
        // recovery event — it still works, just without per-webview filtering.
        let currentWebview: { listen: typeof listen } | null = null
        try {
          const webviewModule = await import('@tauri-apps/api/webview')
          currentWebview = webviewModule.getCurrentWebview()
        } catch (e) {
          console.warn('[useTauriEventBridge] webview API unavailable, falling back to global listen:', e)
        }

        const registerListener = async (
          eventName: string,
          handler: (event: TauriEventPayload) => void,
        ): Promise<boolean> => {
          const unlisten = await listen(eventName, handler)
          if (disposed) {
            unlisten()
            throw new DisposedDuringRegistration()
          }
          pendingUnlistenCallbacks.push(unlisten)
          return true
        }

        const registerWebviewListener = async (
          eventName: string,
          handler: (event: TauriEventPayload) => void,
        ): Promise<boolean> => {
          // Fall back to the global listen if the webview API is unavailable
          // (still functional, just without per-webview filtering).
          const listenFn = currentWebview?.listen ?? listen
          const unlisten = await listenFn(eventName, handler)
          if (disposed) {
            unlisten()
            throw new DisposedDuringRegistration()
          }
          pendingUnlistenCallbacks.push(unlisten)
          return true
        }

        if (
          !(await registerListener('navigate', (event: TauriEventPayload) => {
            if (isRoutePath(event.payload)) {
              navigateTo(event.payload)
            }
          }))
        ) {
          return
        }

        // Tray "Settings", "AI Automation Preferences" and "Automation Page"
        // all emit the single `navigate` event above with a deep-link path.
        // Earlier revisions wired dedicated `tray-toggle-automation` /
        // `automation:quick-access` events, but the only extra behaviour they
        // added was an `automationStatus` query invalidation that the target
        // routes did not consume, so the indirection was pure drag. Keep
        // custom events here only when they carry side effects the target
        // route genuinely needs (e.g. `tray-approve-update` below pairs a
        // navigation with an `updateStatus` refetch).

        if (
          !(await registerListener('tray-approve-update', () => {
            refreshUpdateStatus()
            navigateTo('/updates')
          }))
        ) {
          return
        }

        if (
          !(await registerListener('tray-defer-update', () => {
            refreshUpdateStatus()
            navigateTo('/updates')
          }))
        ) {
          return
        }

        if (
          !(await registerListener('oauth-reauth-required', () => {
            addToast('warning', tRef.current('settingsOAuth.reauthDescription'), 8000)
            navigateTo('/settings')
          }))
        ) {
          return
        }

        if (
          !(await registerListener('navigate:chat', (event: TauriEventPayload) => {
            if (!isChatPayload(event.payload)) {
              navigateTo('/chat')
              return
            }
            const sid = event.payload.sessionId
            if (sid) {
              navigateTo(`/chat?sid=${encodeURIComponent(sid)}`)
            } else {
              navigateTo('/chat')
            }
          }))
        ) {
          return
        }

        if (
          !(await registerListener('integration-proactive-prompt', (event: TauriEventPayload) => {
            if (!isIntegrationPromptPayload(event.payload)) {
              return
            }
            const title = typeof event.payload.title === 'string' ? event.payload.title.trim() : ''
            const body = typeof event.payload.body === 'string' ? event.payload.body.trim() : ''
            const message = [title, body].filter(Boolean).join(': ')
            if (message.length > 0) {
              addToast('info', message, 10000)
            }
          }))
        ) {
          return
        }

        // frontend-recovery is registered via the webview-scoped listener so
        // that emit_to(EventTarget::webview("main"), ...) on the Rust side
        // actually filters delivery to this webview only. With the global
        // `listen()` the listener target is `Any` and emit_to broadcasts to
        // all matching listeners regardless of label.
        if (
          !(await registerWebviewListener('frontend-recovery', (event: TauriEventPayload) => {
            if (!isRecoveryPayload(event.payload)) return
            const { strategy, route, reason } = event.payload
            if (strategy === 'full-reload') {
              console.warn(`[recovery] full-reload: ${reason}`)
              window.location.reload()
              return
            }
            if (strategy === 'reset-route') {
              // Notify the route's error boundary via the typed registry.
              // The boundary handles its own query invalidation and remount;
              // we don't invalidateQueries globally here (avoids IA-3 double).
              notifyRouteRecovery(route)
            }
          }))
        ) {
          return
        }

        unlistenCallbacks = pendingUnlistenCallbacks
      } catch (e) {
        // Drain any listeners that registered before the failure to avoid
        // leaks. Catches three cases:
        //  1. Browser mode or unavailable Tauri event bridge (silent OK)
        //  2. DisposedDuringRegistration sentinel — also silent OK
        //  3. A real listener registration failure — log for debugging
        //
        // Use `instanceof` (not `constructor.name`) so the check survives
        // minification — esbuild renames the class identifier but the
        // prototype chain is preserved.
        for (const unlisten of pendingUnlistenCallbacks) unlisten()
        pendingUnlistenCallbacks = []
        if (e instanceof Error && !(e instanceof DisposedDuringRegistration)) {
          console.warn('[useTauriEventBridge] listener registration failed:', e)
        }
      }
    }

    void registerListeners()

    return () => {
      disposed = true
      for (const unlisten of unlistenCallbacks) unlisten()
      unlistenCallbacks = []
    }
  }, [])
}
