import { useCallback, useSyncExternalStore } from 'react'

export type ToastType = 'success' | 'error' | 'info' | 'warning'

export interface Toast {
  id: string
  type: ToastType
  message: string
  duration: number
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

export function addToast(type: ToastType, message: string, duration = 4000) {
  const id = `toast-${++nextId}`
  store.toasts = [...store.toasts, { id, type, message, duration }]
  notify()

  if (duration > 0) {
    const timeoutId = globalThis.setTimeout(() => {
      removeToast(id)
    }, duration)
    store.timeouts.set(id, timeoutId)
  }

  return id
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
