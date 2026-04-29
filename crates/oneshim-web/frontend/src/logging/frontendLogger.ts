import { IS_TAURI } from '../utils/platform'

type FrontendLogLevel = 'trace' | 'debug' | 'info' | 'warn' | 'error'

type WindowWithFrontendBridge = Window & {
  __oneshimFrontendBridgeInstalled__?: boolean
}

const MAX_MESSAGE_LEN = 2000
const MAX_CONTEXT_LEN = 8000

let coreInvokePromise: Promise<typeof import('@tauri-apps/api/core').invoke> | null = null

function getOriginalConsoleMethod(level: 'warn' | 'error') {
  return console[level].bind(console)
}

function serializeValue(value: unknown): string {
  if (value instanceof Error) {
    return [value.name, value.message, value.stack].filter(Boolean).join(': ')
  }
  if (typeof value === 'string') return value
  if (
    typeof value === 'number' ||
    typeof value === 'boolean' ||
    typeof value === 'bigint' ||
    typeof value === 'symbol'
  ) {
    return String(value)
  }
  if (value == null) return String(value)
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return Object.prototype.toString.call(value)
  }
}

function truncate(value: string, limit: number): string {
  if (value.length <= limit) return value
  return `${value.slice(0, limit)} …(truncated)`
}

function normalizePayload(args: unknown[]): { message: string; context?: string } {
  if (args.length === 0) {
    return { message: 'console call without arguments' }
  }

  const [first, ...rest] = args
  const message = truncate(serializeValue(first).trim() || 'empty console payload', MAX_MESSAGE_LEN)
  const context = rest.map(serializeValue).filter(Boolean).join('\n')

  return {
    message,
    context: context ? truncate(context, MAX_CONTEXT_LEN) : undefined,
  }
}

async function invokeDesktopLog(
  surface: string,
  level: FrontendLogLevel,
  message: string,
  context?: string,
): Promise<void> {
  if (!IS_TAURI) return

  if (!coreInvokePromise) {
    coreInvokePromise = import('@tauri-apps/api/core').then((mod) => mod.invoke)
  }

  const invoke = await coreInvokePromise
  await invoke('record_frontend_log', {
    surface,
    level,
    message,
    context: context ?? null,
  })
}

export function logFrontend(surface: string, level: FrontendLogLevel, message: string, context?: string): void {
  void invokeDesktopLog(surface, level, truncate(message, MAX_MESSAGE_LEN), context).catch((error) => {
    getOriginalConsoleMethod('warn')('record_frontend_log failed:', error)
  })
}

export function installFrontendLogBridge(surface: string): void {
  if (!IS_TAURI || typeof window === 'undefined') return

  const patchedWindow = window as WindowWithFrontendBridge
  if (patchedWindow.__oneshimFrontendBridgeInstalled__) return
  patchedWindow.__oneshimFrontendBridgeInstalled__ = true

  const originalWarn = getOriginalConsoleMethod('warn')
  const originalError = getOriginalConsoleMethod('error')

  console.warn = (...args: unknown[]) => {
    originalWarn(...args)
    const { message, context } = normalizePayload(args)
    logFrontend(surface, 'warn', message, context)
  }

  console.error = (...args: unknown[]) => {
    originalError(...args)
    const { message, context } = normalizePayload(args)
    logFrontend(surface, 'error', message, context)
  }

  window.addEventListener('error', (event) => {
    const message = event.message?.trim() || 'Unhandled window error'
    const context = [event.filename, event.lineno, event.colno, serializeValue(event.error)]
      .filter((value) => value !== '' && value != null)
      .join(' | ')
    logFrontend(surface, 'error', message, context || undefined)
  })

  window.addEventListener('unhandledrejection', (event) => {
    logFrontend(surface, 'error', 'Unhandled promise rejection', serializeValue(event.reason))
  })
}
