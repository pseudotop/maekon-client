/**
 * Tauri app launcher for WebdriverIO E2E tests.
 *
 * Spawns the oneshim binary with the webdriver feature enabled,
 * waits for the WebDriver server to become ready, and provides
 * lifecycle management (start/stop).
 */
import { type ChildProcess, execFileSync, spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { createConnection } from 'node:net'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

let appProcess: ChildProcess | null = null

/** Check if a port is already in use. */
async function isPortInUse(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const conn = createConnection({ port, host: '127.0.0.1' })
    conn.once('connect', () => {
      conn.destroy()
      resolve(true)
    })
    conn.once('error', () => resolve(false))
    conn.setTimeout(500, () => {
      conn.destroy()
      resolve(false)
    })
  })
}

/** Kill any leftover oneshim processes. */
function killStaleProcesses(): void {
  try {
    execFileSync('pkill', ['-f', 'target/(debug|release)/oneshim'], { stdio: 'ignore' })
  } catch {
    // Ignore — no stale processes
  }
}

/** Locate the built binary. Prefers debug build, falls back to release. */
function findBinary(): string {
  const root = resolve(__dirname, '../../../..')
  const candidates = [resolve(root, 'target/debug/oneshim'), resolve(root, 'target/release/oneshim')]
  for (const bin of candidates) {
    if (existsSync(bin)) return bin
  }
  throw new Error(
    `oneshim binary not found. Run: cargo build -p oneshim-app --features webdriver\n` +
      `Checked: ${candidates.join(', ')}`,
  )
}

/** Start the Tauri app and wait for WebDriver server readiness. */
export async function startApp(webdriverPort = 4445, timeoutSec = 30): Promise<void> {
  if (appProcess) return

  // Pre-flight: check for port conflicts
  if (await isPortInUse(webdriverPort)) {
    console.warn(`[e2e-tauri] Port ${webdriverPort} already in use. Killing stale processes...`)
    killStaleProcesses()
    await new Promise((r) => setTimeout(r, 1000))
    if (await isPortInUse(webdriverPort)) {
      throw new Error(
        `Port ${webdriverPort} still in use after cleanup.\n` + `Fix: lsof -ti:${webdriverPort} | xargs kill -9`,
      )
    }
  }

  const binary = findBinary()
  console.log(`[e2e-tauri] Starting: ${binary}`)
  console.log(`[e2e-tauri] WebDriver port: ${webdriverPort}`)

  appProcess = spawn(binary, [], {
    env: {
      ...process.env,
      TAURI_WEBDRIVER_PORT: String(webdriverPort),
      ONESHIM_DISABLE_TRAY: '1',
      RUST_LOG: 'oneshim=info,tauri_plugin_webdriver=debug',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  })

  appProcess.stdout?.on('data', (data: Buffer) => {
    const line = data.toString().trim()
    if (line) console.log(`[app:stdout] ${line}`)
  })
  appProcess.stderr?.on('data', (data: Buffer) => {
    const line = data.toString().trim()
    if (line) console.log(`[app:stderr] ${line}`)
  })
  appProcess.on('exit', (code) => {
    console.log(`[e2e-tauri] App exited with code ${code}`)
    appProcess = null
  })

  // Poll WebDriver /status endpoint until ready
  const url = `http://127.0.0.1:${webdriverPort}/status`
  const start = Date.now()
  const deadline = start + timeoutSec * 1000

  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) })
      if (res.ok) {
        const body = await res.json()
        if (body?.value?.ready) {
          const elapsed = ((Date.now() - start) / 1000).toFixed(1)
          console.log(`[e2e-tauri] WebDriver ready after ${elapsed}s`)
          return
        }
      }
    } catch {
      // Not ready yet
    }
    await new Promise((r) => setTimeout(r, 500))
  }

  stopApp()
  throw new Error(`WebDriver server not ready after ${timeoutSec}s`)
}

/** Terminate the Tauri app. */
export function stopApp(): void {
  if (!appProcess) return
  console.log(`[e2e-tauri] Stopping app (PID: ${appProcess.pid})`)
  appProcess.kill('SIGTERM')
  // Escalate after 3s if still alive
  setTimeout(() => {
    if (appProcess) {
      console.log('[e2e-tauri] Escalating to SIGKILL')
      appProcess.kill('SIGKILL')
    }
  }, 3000)
  appProcess = null
}
