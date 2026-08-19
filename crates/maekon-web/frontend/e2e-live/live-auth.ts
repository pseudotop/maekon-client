const LOCAL_AUTH_ENV = 'MAEKON_LOCAL_AUTH_TOKEN'
const WEBDRIVER_PORT_ENV = 'TAURI_WEBDRIVER_PORT'

type Environment = Record<string, string | undefined>

type ResolveOptions = {
  env?: Environment
  fetchImpl?: typeof fetch
}

type WebDriverEnvelope<T> = {
  value?: T
  sessionId?: string
}

function validateToken(raw: string | undefined): string | null {
  const token = raw?.trim() ?? ''
  if (!token) return null
  if (!/^[A-Za-z0-9._~-]{16,256}$/.test(token)) {
    throw new Error(`${LOCAL_AUTH_ENV} has an invalid format`)
  }
  return token
}

async function webdriverJson<T>(
  fetchImpl: typeof fetch,
  url: string,
  init?: RequestInit,
): Promise<WebDriverEnvelope<T>> {
  const response = await fetchImpl(url, init)
  if (!response.ok) {
    throw new Error(`WebDriver request failed with HTTP ${response.status}`)
  }
  return (await response.json()) as WebDriverEnvelope<T>
}

async function resolveFromWebDriver(fetchImpl: typeof fetch, port: number): Promise<string> {
  const base = `http://127.0.0.1:${port}`
  const session = await webdriverJson<{ sessionId?: string }>(fetchImpl, `${base}/session`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ capabilities: { alwaysMatch: {} } }),
  })
  const sessionId = session.value?.sessionId ?? session.sessionId
  if (!sessionId) throw new Error('WebDriver did not return a session id')

  const sessionBase = `${base}/session/${sessionId}`
  try {
    const handles = await webdriverJson<string[]>(fetchImpl, `${sessionBase}/window/handles`)
    const handle = handles.value?.find((candidate) => candidate === 'main') ?? handles.value?.[0]
    if (!handle) throw new Error('WebDriver did not expose a Maekon window')

    await webdriverJson<null>(fetchImpl, `${sessionBase}/window`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ handle }),
    })

    const tokenResponse = await webdriverJson<string>(fetchImpl, `${sessionBase}/execute/sync`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        // DesktopStartupCoordinator injects the process-lifetime token before
        // showing the trusted main WebView. Reading that established contract
        // avoids nested Tauri IPC from a WebDriver async script; on Windows the
        // nested call serialized a null callback identifier into a required u32.
        script: 'return window.__MAEKON_LOCAL_AUTH__ ?? null;',
        args: [],
      }),
    })
    const token = validateToken(tokenResponse.value)
    if (!token) throw new Error('Tauri returned an empty local-auth token')
    return token
  } finally {
    await fetchImpl(sessionBase, { method: 'DELETE' }).catch(() => undefined)
  }
}

export async function resolveLiveAuthToken(options: ResolveOptions = {}): Promise<string> {
  const env = options.env ?? process.env
  const explicitToken = validateToken(env[LOCAL_AUTH_ENV])
  if (explicitToken) return explicitToken

  const rawPort = env[WEBDRIVER_PORT_ENV]?.trim() || '4445'
  const port = Number(rawPort)
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error(`${WEBDRIVER_PORT_ENV} must be a valid TCP port`)
  }

  try {
    return await resolveFromWebDriver(options.fetchImpl ?? fetch, port)
  } catch (error) {
    const reason = error instanceof Error ? error.message : 'unknown WebDriver error'
    throw new Error(
      `Live E2E local authentication is unavailable (${reason}). Set ${LOCAL_AUTH_ENV} or run a webdriver-enabled Maekon app on ${WEBDRIVER_PORT_ENV}=${port}.`,
    )
  }
}

export function requireLiveAuthToken(env: Environment = process.env): string {
  const token = validateToken(env[LOCAL_AUTH_ENV])
  if (!token) {
    throw new Error(`${LOCAL_AUTH_ENV} was not initialized by the Playwright global setup`)
  }
  return token
}
