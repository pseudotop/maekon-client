import { DEFAULT_WEB_PORT } from '../constants'
import { IS_TAURI } from './platform'

/**
 * In Tauri webview, the frontend is served via tauri:// protocol.
 * Relative URLs like /api/stream won't reach the Axum backend at 127.0.0.1.
 * Use absolute URLs when running inside Tauri.
 *
 * The web port is resolved in this order:
 * 1. window.__ONESHIM_WEB_PORT__ (injected by setup.rs via eval before page loads)
 * 2. Tauri IPC get_web_port command (async, updates URLs after init)
 * 3. DEFAULT_WEB_PORT fallback (10090)
 */
declare global {
  interface Window {
    __ONESHIM_WEB_PORT__?: number
  }
}

let resolvedPort = (typeof window !== 'undefined' && window.__ONESHIM_WEB_PORT__) || DEFAULT_WEB_PORT

let webPortPromise: Promise<number> | null = null

function setResolvedPort(port: number): number {
  if (!Number.isFinite(port) || port <= 0) {
    return resolvedPort
  }
  resolvedPort = port
  if (typeof window !== 'undefined') {
    window.__ONESHIM_WEB_PORT__ = port
  }
  return resolvedPort
}

export function getResolvedWebPort(): number {
  return resolvedPort
}

export function getApiBaseUrl(): string {
  return IS_TAURI ? `http://127.0.0.1:${resolvedPort}/api` : '/api'
}

export function getSseStreamUrl(): string {
  return IS_TAURI ? `http://127.0.0.1:${resolvedPort}/api/stream` : '/api/stream'
}

export function getUpdateStreamUrl(): string {
  return IS_TAURI ? `http://127.0.0.1:${resolvedPort}/api/update/stream` : '/api/update/stream'
}

export async function resolveWebPort(): Promise<number> {
  if (!IS_TAURI) {
    return resolvedPort
  }

  if (!webPortPromise) {
    webPortPromise = import('@tauri-apps/api/core')
      .then(({ invoke }) => invoke<number>('get_web_port'))
      .then((port) => {
        console.debug(`web port resolved: ${port}`)
        return setResolvedPort(port)
      })
      .catch((e) => {
        console.debug('get_web_port failed, using fallback port:', e)
        webPortPromise = null
        return resolvedPort
      })
  }

  return webPortPromise
}

export async function resolveApiUrl(url: string): Promise<string> {
  if (!IS_TAURI || !url.startsWith('/api')) {
    return url
  }

  const port = await resolveWebPort()
  return `http://127.0.0.1:${port}${url}`
}

/**
 * Synchronous image URL resolver for <img src> attributes.
 * In Tauri mode, converts relative /api/* paths to absolute URLs using
 * the already-resolved port. In browser mode, returns the path as-is.
 */
export function resolveImageUrl(url: string | null | undefined): string | null {
  if (!url) return null
  if (!IS_TAURI || !url.startsWith('/api')) return url
  return `http://127.0.0.1:${resolvedPort}${url}`
}

if (IS_TAURI) {
  void resolveWebPort().catch((e) => {
    console.debug('resolveWebPort init failed, using default port:', e)
  })
}
