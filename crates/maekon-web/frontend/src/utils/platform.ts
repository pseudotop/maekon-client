/**
 * Platform detection utility.
 * Evaluated once at module load — safe for use in components without re-computation.
 */

interface NavigatorUAData {
  platform: string
}

interface TauriRuntimeGlobal {
  __TAURI_INTERNALS__?: unknown
  isTauri?: boolean
}

const nav = typeof navigator !== 'undefined' ? navigator : undefined
const uaData =
  nav && 'userAgentData' in nav ? (nav as Navigator & { userAgentData?: NavigatorUAData }).userAgentData : undefined

export const IS_MAC = uaData ? uaData.platform === 'macOS' : /mac/i.test(nav?.platform ?? '')
export const IS_WINDOWS = uaData ? /win/i.test(uaData.platform) : /win/i.test(nav?.platform ?? '')
export const IS_LINUX = uaData ? /linux/i.test(uaData.platform) : /linux/i.test(nav?.platform ?? '')

export function isTauriRuntime(): boolean {
  if (typeof window === 'undefined') {
    return false
  }

  const runtime = globalThis as TauriRuntimeGlobal
  const host = window.location.hostname
  return (
    runtime.isTauri === true ||
    '__TAURI_INTERNALS__' in window ||
    window.location.protocol === 'tauri:' ||
    host === 'tauri.localhost'
  )
}

export const IS_TAURI = isTauriRuntime()

export const MOD_KEY = IS_MAC ? '\u2318' : 'Ctrl'
