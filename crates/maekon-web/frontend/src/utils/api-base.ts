import { DEFAULT_WEB_PORT } from '../constants'
import { IS_TAURI } from './platform'

/**
 * In Tauri webview, the frontend is served via tauri:// protocol.
 * Relative URLs like /api/stream won't reach the Axum backend at 127.0.0.1.
 * Use absolute URLs when running inside Tauri.
 *
 * The web port is resolved in this order:
 * 1. window.__MAEKON_WEB_PORT__ (injected by setup.rs via eval before page loads)
 * 2. Tauri IPC get_web_port command (async, updates URLs after init)
 * 3. DEFAULT_WEB_PORT fallback (10090)
 */
declare global {
  interface Window {
    __MAEKON_WEB_PORT__?: number
    // E20-41 (#4833): per-session local-API auth token, injected by setup.rs into
    // the legit Tauri WebView (or set from a ?local_auth= browser handshake).
    __MAEKON_LOCAL_AUTH__?: string
  }
}

let resolvedPort = (typeof window !== 'undefined' && window.__MAEKON_WEB_PORT__) || DEFAULT_WEB_PORT

let webPortPromise: Promise<number> | null = null

function setResolvedPort(port: number): number {
  if (!Number.isFinite(port) || port <= 0) {
    return resolvedPort
  }
  resolvedPort = port
  if (typeof window !== 'undefined') {
    window.__MAEKON_WEB_PORT__ = port
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

// ── E20-41 (#4833): per-session local-API auth token ────────────────────────
// The internal /api requires this token (X-Local-Auth header for fetch, or the
// maekon_local_auth cookie for EventSource/SSE which cannot set headers). The
// legit client receives it via WebView injection or the get_local_auth_token IPC;
// a different local user's browser never has it and is rejected (401).

let resolvedToken = (typeof window !== 'undefined' && window.__MAEKON_LOCAL_AUTH__) || ''
let tokenPromise: Promise<string> | null = null

export function getLocalAuthToken(): string {
  // Re-read the injected global lazily — setup.rs injects it post-load (eval).
  if (!resolvedToken && typeof window !== 'undefined' && window.__MAEKON_LOCAL_AUTH__) {
    resolvedToken = window.__MAEKON_LOCAL_AUTH__
  }
  return resolvedToken
}

/**
 * Set the `maekon_local_auth` cookie so EventSource/SSE (which cannot attach
 * custom headers) authenticates to /api. Path=/api scopes it to the API surface;
 * SameSite=Strict blocks cross-site sends; the hex token needs no URL-encoding.
 */
export function setLocalAuthCookie(): void {
  if (typeof document === 'undefined') return
  const token = getLocalAuthToken()
  if (!token) return
  const secure = typeof location !== 'undefined' && location.protocol === 'https:' ? '; Secure' : ''
  // biome-ignore lint/suspicious/noDocumentCookie: EventSource cannot attach custom headers; the cookie is the only SSE auth channel.
  document.cookie = `maekon_local_auth=${token}; Path=/api; SameSite=Strict${secure}`
}

/** Attach the X-Local-Auth header to a fetch init (the single fetch chokepoint). */
export function withLocalAuthHeaders(init?: RequestInit): RequestInit {
  const token = getLocalAuthToken()
  if (!token) return init ?? {}
  const headers = new Headers(init?.headers)
  headers.set('X-Local-Auth', token)
  return { ...init, headers }
}

/** Await Tauri token resolution before building fetch headers. */
export async function withResolvedLocalAuthHeaders(
  init?: RequestInit,
  options?: { forceRefresh?: boolean },
): Promise<RequestInit> {
  await resolveLocalAuthToken(options)
  return withLocalAuthHeaders(init)
}

/**
 * Append the local-auth token as a `?local_auth=` query param only when the
 * EventSource request cannot rely on the same-origin API cookie. Tauri WebView
 * streams are cross-origin (tauri://localhost → 127.0.0.1), custom headers are
 * impossible, and the tauri-document cookie is never sent to the 127.0.0.1
 * origin. Same-origin browser streams use the `maekon_local_auth` cookie instead
 * so the token does not travel in request URLs. The token is hex, so no
 * URL-encoding is needed (and the server compares the raw value).
 */
export function withLocalAuthQuery(url: string): string {
  const token = getLocalAuthToken()
  if (!token) return url
  if (!shouldUseLocalAuthQuery(url)) return url
  return `${url}${url.includes('?') ? '&' : '?'}local_auth=${token}`
}

function shouldUseLocalAuthQuery(url: string): boolean {
  if (IS_TAURI) return true
  if (typeof location === 'undefined') return false
  try {
    return new URL(url, location.origin).origin !== location.origin
  } catch {
    return true
  }
}

/** Race-safe token resolver: injected global first, then the get_local_auth_token IPC. */
export async function resolveLocalAuthToken(options?: { forceRefresh?: boolean }): Promise<string> {
  if (!options?.forceRefresh && getLocalAuthToken()) return resolvedToken
  if (!IS_TAURI) return ''
  if (options?.forceRefresh) {
    resolvedToken = ''
    if (typeof window !== 'undefined') {
      delete window.__MAEKON_LOCAL_AUTH__
    }
    tokenPromise = null
  }
  if (!tokenPromise) {
    tokenPromise = import('@tauri-apps/api/core')
      .then(({ invoke }) => invoke<string>('get_local_auth_token'))
      .then((token) => {
        resolvedToken = token || ''
        if (typeof window !== 'undefined') window.__MAEKON_LOCAL_AUTH__ = resolvedToken
        setLocalAuthCookie()
        return resolvedToken
      })
      .catch((e) => {
        console.debug('get_local_auth_token failed:', e)
        tokenPromise = null
        return resolvedToken
      })
  }
  return tokenPromise
}

// Plain-browser handshake: a surfaced URL may carry the token in the URL FRAGMENT
// (#local_auth=<token>). A fragment is NEVER sent to the server, so it cannot leak
// into access logs or the Referer header (unlike a ?query=). Read it, set the
// same-origin cookie, then scrub the fragment from history.
if (typeof window !== 'undefined' && typeof location !== 'undefined' && location.hash) {
  const hashParams = new URLSearchParams(location.hash.replace(/^#/, ''))
  const handshakeToken = hashParams.get('local_auth')
  if (handshakeToken) {
    window.__MAEKON_LOCAL_AUTH__ = handshakeToken
    resolvedToken = handshakeToken
    setLocalAuthCookie()
    hashParams.delete('local_auth')
    const rest = hashParams.toString()
    window.history.replaceState(null, '', `${location.pathname}${location.search}${rest ? `#${rest}` : ''}`)
  }
}

if (IS_TAURI) {
  void resolveWebPort().catch((e) => {
    console.debug('resolveWebPort init failed, using default port:', e)
  })
  void resolveLocalAuthToken().catch((e) => {
    console.debug('resolveLocalAuthToken init failed:', e)
  })
} else {
  // Browser mode: the token may already be present (handshake/injection) — make
  // sure the SSE cookie is set for EventSource consumers.
  setLocalAuthCookie()
}
