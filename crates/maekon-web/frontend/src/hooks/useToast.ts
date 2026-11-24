import { useCallback, useSyncExternalStore } from 'react'
import type { ErrorDisplay } from '../i18n/errorDisplay'

export type ToastType = 'success' | 'error' | 'info' | 'warning'

export interface Toast {
  id: string
  type: ToastType
  message: string
  duration: number
  /** Untranslated support text, rendered collapsed under the message (#13737). */
  detail?: string | null
  /** Opt-in live localization of an error; never persisted or logged. */
  errorSource?: { readonly cause: unknown; readonly fallbackKey?: string }
}

interface ToastStore {
  toasts: Toast[]
  listeners: Set<() => void>
  timeouts: Map<string, ReturnType<typeof setTimeout>>
}

const store: ToastStore = {
  toasts: [],
  listeners: new Set(),
  timeouts: new Map(),
}

let nextId = 0

function notify() {
  store.listeners.forEach((listener) => {
    listener()
  })
}

function subscribe(listener: () => void) {
  store.listeners.add(listener)
  return () => store.listeners.delete(listener)
}

function getSnapshot() {
  return store.toasts
}

function clearToastTimeout(id: string) {
  const timeoutId = store.timeouts.get(id)
  if (timeoutId === undefined) {
    return
  }

  globalThis.clearTimeout(timeoutId)
  store.timeouts.delete(id)
}

export function removeToast(id: string) {
  clearToastTimeout(id)
  store.toasts = store.toasts.filter((toast) => toast.id !== id)
  notify()
}

export function clearToasts() {
  store.timeouts.forEach((timeoutId) => {
    globalThis.clearTimeout(timeoutId)
  })
  store.timeouts.clear()
  store.toasts = []
  notify()
}

/**
 * Keep a toast on screen until the user closes it. Opening a toast's support
 * details calls this so the panel does not vanish while someone reads or copies
 * it.
 */
export function pinToast(id: string) {
  clearToastTimeout(id)
}

export function addToast(
  type: ToastType,
  message: string,
  duration = 4000,
  detail: string | null = null,
  errorSource?: Toast['errorSource'],
) {
  const id = `toast-${++nextId}`
  store.toasts = [
    ...store.toasts,
    { id, type, message, duration, ...(detail ? { detail } : {}), ...(errorSource ? { errorSource } : {}) },
  ]
  notify()

  if (duration > 0) {
    const timeoutId = globalThis.setTimeout(() => {
      removeToast(id)
    }, duration)
    store.timeouts.set(id, timeoutId)
  }

  return id
}

/**
 * Error toast for a failure already resolved by `describeErrorForDisplay` /
 * `useErrorDisplay`: the localized sentence, with any untranslated detail kept
 * behind the toast's support-details panel. Failures may retain their
 * source to retranslate in-place after a locale change, even after the owning
 * screen unmounts. Existing surface-specific display/fallback calls stay fixed.
 */
export function addErrorToast(display: ErrorDisplay, duration = 5000, errorSource?: Toast['errorSource']) {
  return addToast('error', display.text, duration, display.detail, errorSource)
}

export function useToast() {
  const toasts = useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
  const show = useCallback(
    (type: ToastType, message: string, duration?: number) => addToast(type, message, duration),
    [],
  )

  return {
    toasts,
    show,
    remove: removeToast,
    clear: clearToasts,
  }
}
