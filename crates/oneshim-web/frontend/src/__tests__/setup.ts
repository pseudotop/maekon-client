import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterEach, vi } from 'vitest'

afterEach(() => {
  cleanup()
})

// Provide __APP_VERSION__ global used by StatusBar
;(globalThis as Record<string, unknown>).__APP_VERSION__ = 'v0.1.0-test'

// Mock EventSource (jsdom doesn't provide it)
class MockEventSource {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSED = 2

  readonly CONNECTING = 0
  readonly OPEN = 1
  readonly CLOSED = 2

  url: string
  readyState: number = MockEventSource.CONNECTING
  onopen: ((ev: Event) => void) | null = null
  onmessage: ((ev: MessageEvent) => void) | null = null
  onerror: ((ev: Event) => void) | null = null
  withCredentials = false

  private listeners = new Map<string, Set<EventListener>>()

  constructor(url: string | URL) {
    this.url = typeof url === 'string' ? url : url.toString()
  }

  addEventListener(type: string, listener: EventListener) {
    if (!this.listeners.has(type)) {
      this.listeners.set(type, new Set())
    }
    this.listeners.get(type)?.add(listener)
  }

  removeEventListener(type: string, listener: EventListener) {
    this.listeners.get(type)?.delete(listener)
  }

  dispatchEvent(_event: Event): boolean {
    return true
  }

  close() {
    this.readyState = MockEventSource.CLOSED
  }
}

Object.defineProperty(globalThis, 'EventSource', {
  value: MockEventSource,
  writable: true,
})

// Stub matchMedia for ThemeProvider
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockImplementation((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
})
