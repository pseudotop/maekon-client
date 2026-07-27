// e2e-tauri/helpers.ts

type TauriBridge = {
  invoke<T = unknown>(command: string, args?: Record<string, unknown>): Promise<T>
}

type MaekonWindow = Window & {
  __MAEKON_WEB_PORT__?: number
  __TAURI_INTERNALS__?: TauriBridge
}

/**
 * Tauri IPC command invocation — wraps window.__TAURI_INTERNALS__.invoke()
 * Runs inside the browser context via WebdriverIO's executeAsync
 */
export async function invokeIpc<T = unknown>(command: string, args?: Record<string, unknown>): Promise<T> {
  const result = await browser.executeAsync(
    (
      cmd: string,
      cmdArgs: Record<string, unknown> | undefined,
      done: (r: { ok: boolean; data?: unknown; error?: string }) => void,
    ) => {
      const tauri = (window as unknown as MaekonWindow).__TAURI_INTERNALS__
      if (!tauri) {
        done({ ok: false, error: 'TAURI_INTERNALS not available' })
        return
      }
      tauri
        .invoke(cmd, cmdArgs || {})
        .then((data: unknown) => done({ ok: true, data }))
        .catch((err: unknown) => done({ ok: false, error: String(err) }))
    },
    command,
    args,
  )
  if (!result.ok) {
    throw new Error(`IPC ${command} failed: ${result.error}`)
  }
  return result.data as T
}

export async function switchToMainWindow(): Promise<void> {
  await browser.waitUntil(
    async () => {
      const handles = await browser.getWindowHandles()
      for (const handle of handles) {
        await browser.switchToWindow(handle)
        const title = await browser.getTitle().catch(() => '')
        const url = await browser.getUrl().catch(() => '')
        const isAuxiliaryWindow = url.includes('overlay') || url.includes('tracking-panel')
        if (title.includes('Maekon') && !isAuxiliaryWindow) return true
      }
      return false
    },
    { timeout: 15000, timeoutMsg: 'Maekon main window was not available' },
  )
}

export async function navigateMain(pathname = '/'): Promise<void> {
  await switchToMainWindow()
  const nextPath = pathname.startsWith('/') ? pathname : `/${pathname}`
  // Keep the live Tauri document and IPC bridge intact. A top-level WebDriver
  // navigation can turn a custom-scheme page into an external WebView load on
  // Windows; BrowserRouter only needs a history entry plus popstate.
  await browser.execute((path: string) => {
    window.history.pushState({}, '', path)
    window.dispatchEvent(new PopStateEvent('popstate'))
  }, nextPath)
  await $('body').waitForExist({ timeout: 10000 })
}

export async function ensureShellReady(): Promise<void> {
  const body = await $('body')
  await body.waitForExist({ timeout: 10000 })

  await browser.waitUntil(
    async () => {
      const statusBar = await $('.app-shell-statusbar')
      const skipButton = await $('[data-testid="onboarding-skip"]')
      return (await statusBar.isExisting()) || (await skipButton.isExisting())
    },
    { timeout: 15000, timeoutMsg: 'App shell or onboarding did not render' },
  )

  if (await (await $('.app-shell-statusbar')).isExisting()) return

  const skipButton = await $('[data-testid="onboarding-skip"]')
  if (await skipButton.isExisting()) {
    await skipButton.waitForClickable({ timeout: 10000 })
    await skipButton.click()
  } else {
    await invokeIpc('complete_onboarding')
    await browser.execute(() => window.location.reload())
  }
  await $('.app-shell-statusbar').waitForExist({ timeout: 15000 })
}

export async function fetchApiJson<T = unknown>(
  path: string,
  init?: {
    method?: string
    headers?: Record<string, string>
    body?: string
  },
): Promise<T> {
  const normalizedPath = path.startsWith('/') ? path : `/${path}`
  const result = await browser.executeAsync(
    (
      relativePath: string,
      requestInit: { method?: string; headers?: Record<string, string>; body?: string } | undefined,
      done: (r: { ok: boolean; data?: unknown; error?: string }) => void,
    ) => {
      const appWindow = window as unknown as MaekonWindow
      const port = appWindow.__MAEKON_WEB_PORT__ || 10090
      const url = `http://127.0.0.1:${port}/api${relativePath}`
      const tauri = appWindow.__TAURI_INTERNALS__
      if (!tauri) {
        done({ ok: false, error: 'TAURI_INTERNALS not available' })
        return
      }

      tauri
        .invoke<string>('get_local_auth_token')
        .then((token: string) => {
          const headers = new Headers(requestInit?.headers)
          headers.set('x-local-auth', token)
          return fetch(url, { ...requestInit, headers })
        })
        .then(async (response: Response) => {
          const text = await response.text()
          let data: unknown = null
          if (text.length > 0) {
            try {
              data = JSON.parse(text)
            } catch {
              data = text
            }
          }

          if (!response.ok) {
            done({
              ok: false,
              error:
                typeof data === 'string'
                  ? `${response.status} ${response.statusText}: ${data}`
                  : `${response.status} ${response.statusText}`,
            })
            return
          }

          done({ ok: true, data })
        })
        .catch((err: unknown) => done({ ok: false, error: String(err) }))
    },
    normalizedPath,
    init,
  )
  if (!result.ok) {
    throw new Error(`API ${normalizedPath} failed: ${result.error}`)
  }
  return result.data as T
}

/**
 * Wait for an SSE event — captures a specific event type from /api/stream
 * Uses the WebView's own EventSource, so it operates within the CSP connect-src scope
 */
export async function waitForSseEvent<T = Record<string, unknown>>(eventType: string, timeoutMs = 10000): Promise<T> {
  const result = await browser.executeAsync(
    (type: string, timeout: number, done: (r: { ok: boolean; data?: unknown; error?: string }) => void) => {
      const appWindow = window as unknown as MaekonWindow
      const port = appWindow.__MAEKON_WEB_PORT__ || 10090
      const tauri = appWindow.__TAURI_INTERNALS__
      if (!tauri) {
        done({ ok: false, error: 'TAURI_INTERNALS not available' })
        return
      }

      const controller = new AbortController()
      let finished = false
      const finish = (value: { ok: boolean; data?: unknown; error?: string }) => {
        if (finished) return
        finished = true
        clearTimeout(timer)
        controller.abort()
        done(value)
      }
      const timer = setTimeout(() => {
        finish({ ok: false, error: `SSE ${type} timeout after ${timeout}ms` })
      }, timeout)

      tauri
        .invoke<string>('get_local_auth_token')
        .then((token: string) =>
          fetch(`http://127.0.0.1:${port}/api/stream`, {
            headers: {
              Accept: 'text/event-stream',
              'x-local-auth': token,
            },
            signal: controller.signal,
          }),
        )
        .then(async (response: Response) => {
          if (!response.ok) {
            finish({ ok: false, error: `SSE HTTP ${response.status}` })
            return
          }
          if (!response.body) {
            finish({ ok: false, error: 'SSE response body unavailable' })
            return
          }

          const reader = response.body.getReader()
          const decoder = new TextDecoder()
          let buffer = ''

          while (!finished) {
            const chunk = await reader.read()
            if (chunk.done) break
            buffer += decoder.decode(chunk.value, { stream: true }).replace(/\r\n/g, '\n')

            let separator = buffer.indexOf('\n\n')
            while (separator >= 0) {
              const block = buffer.slice(0, separator)
              buffer = buffer.slice(separator + 2)
              let name = 'message'
              const data: string[] = []
              for (const line of block.split('\n')) {
                if (line.startsWith('event:')) name = line.slice(6).trim()
                if (line.startsWith('data:')) data.push(line.slice(5).trimStart())
              }
              if (name === type) {
                const payload = data.join('\n')
                try {
                  finish({ ok: true, data: JSON.parse(payload) })
                } catch {
                  finish({ ok: true, data: payload })
                }
                return
              }
              separator = buffer.indexOf('\n\n')
            }
          }

          if (!finished) finish({ ok: false, error: 'SSE connection closed before target event' })
        })
        .catch((err: unknown) => {
          if (!finished) finish({ ok: false, error: String(err) })
        })
    },
    eventType,
    timeoutMs,
  )
  if (!result.ok) {
    throw new Error(result.error)
  }
  return result.data as T
}

export interface UpdateStatusResponse {
  phase: string
  message?: string
  latest_version?: string
}
