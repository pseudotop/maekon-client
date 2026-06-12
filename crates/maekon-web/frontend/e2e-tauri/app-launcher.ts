/**
 * Tauri app launcher for WebdriverIO E2E tests.
 *
 * Spawns the maekon binary with the webdriver feature enabled,
 * waits for the WebDriver server to become ready, and provides
 * lifecycle management (start/stop).
 */
import { type ChildProcess, execFileSync, spawn } from 'node:child_process'
import { existsSync, mkdirSync } from 'node:fs'
import { createConnection } from 'node:net'
import { delimiter, dirname, resolve, win32 as winPath } from 'node:path'
import { fileURLToPath } from 'node:url'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

type CommandSpec = {
  file: string
  args: string[]
}

type E2eProfileEnv = {
  APPDATA: string
  LOCALAPPDATA: string
  MAEKON_APP_FLAVOR: string
}

type RuntimePathEnv = {
  Path?: string
  PATH?: string
}

let appProcess: ChildProcess | null = null

function workspaceRoot(): string {
  return resolve(__dirname, '../../../..')
}

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

/** Kill any leftover maekon processes. */
export function staleProcessCleanupCommand(platform: NodeJS.Platform = process.platform): CommandSpec {
  if (platform === 'win32') {
    const script = [
      "$ErrorActionPreference = 'SilentlyContinue'",
      'Get-CimInstance Win32_Process -Filter "name = \'maekon.exe\'"',
      "| Where-Object { $_.ExecutablePath -match '\\\\target\\\\(debug|release)\\\\maekon\\.exe$' }",
      '| ForEach-Object { Stop-Process -Id $_.ProcessId -Force }',
    ].join(' ')

    return {
      file: 'powershell.exe',
      args: ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', script],
    }
  }

  return {
    file: 'pkill',
    args: ['-f', 'target/(debug|release)/maekon'],
  }
}

function killStaleProcesses(): void {
  const { file, args } = staleProcessCleanupCommand()
  try {
    execFileSync(file, args, { stdio: 'ignore' })
  } catch {
    // Ignore — no stale processes
  }
}

/** Return platform-specific candidate binary paths. */
export function binaryCandidates(
  root: string,
  platform: NodeJS.Platform = process.platform,
  cargoTargetDir = process.env.CARGO_TARGET_DIR,
): string[] {
  const extension = platform === 'win32' ? '.exe' : ''
  const profilePaths = [`debug/maekon${extension}`, `release/maekon${extension}`]
  const explicitTargetCandidates = cargoTargetDir
    ? profilePaths.map((profilePath) => resolve(cargoTargetDir, profilePath))
    : []
  const workspaceTargetCandidates = profilePaths.map((profilePath) => resolve(root, 'target', profilePath))

  return [...explicitTargetCandidates, ...workspaceTargetCandidates]
}

/** Return isolated app profile directories for E2E launches. */
export function e2eProfileEnv(root: string, profileRoot = process.env.MAEKON_E2E_PROFILE_ROOT): E2eProfileEnv {
  const base = profileRoot ? resolve(root, profileRoot) : resolve(root, '.evidence', 'e2e-tauri', 'profile')

  return {
    APPDATA: resolve(base, 'roaming'),
    LOCALAPPDATA: resolve(base, 'local'),
    MAEKON_APP_FLAVOR: process.env.MAEKON_APP_FLAVOR ?? 'e2e',
  }
}

function ensureProfileDirs(env: E2eProfileEnv): void {
  mkdirSync(env.APPDATA, { recursive: true })
  mkdirSync(env.LOCALAPPDATA, { recursive: true })
}

/** Return PATH overrides needed by Windows runtime dependencies. */
export function runtimePathEnv(
  platform: NodeJS.Platform = process.platform,
  env: NodeJS.ProcessEnv = process.env,
): RuntimePathEnv {
  const pathKey = platform === 'win32' && env.Path !== undefined ? 'Path' : 'PATH'
  const currentPath = env[pathKey] ?? env.PATH ?? env.Path ?? ''

  if (platform !== 'win32' || !env.OPENSSL_DIR) {
    return { [pathKey]: currentPath }
  }

  const separator = platform === 'win32' ? ';' : delimiter
  const opensslBin = winPath.resolve(env.OPENSSL_DIR, 'bin')
  const hasOpenSslBin = currentPath
    .split(separator)
    .some((part) => winPath.normalize(part).toLowerCase() === winPath.normalize(opensslBin).toLowerCase())

  if (hasOpenSslBin) {
    return { [pathKey]: currentPath }
  }

  return { [pathKey]: currentPath ? `${opensslBin}${separator}${currentPath}` : opensslBin }
}

/** Locate the built binary. Prefers debug build, falls back to release. */
function findBinary(): string {
  const root = workspaceRoot()
  const candidates = binaryCandidates(root)
  for (const bin of candidates) {
    if (existsSync(bin)) return bin
  }
  throw new Error(
    `maekon binary not found. Run: cargo build -p maekon-app --features webdriver\n` +
      `Checked: ${candidates.join(', ')}`,
  )
}

/** Return a Windows process-tree termination command for a launched PID. */
export function terminationCommand(pid: number, platform: NodeJS.Platform = process.platform): CommandSpec | null {
  if (platform !== 'win32') return null
  return {
    file: 'taskkill.exe',
    args: ['/PID', String(pid), '/T', '/F'],
  }
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
  const profileEnv = e2eProfileEnv(workspaceRoot())
  const pathEnv = runtimePathEnv()
  ensureProfileDirs(profileEnv)

  console.log(`[e2e-tauri] Starting: ${binary}`)
  console.log(`[e2e-tauri] WebDriver port: ${webdriverPort}`)
  console.log(`[e2e-tauri] APPDATA: ${profileEnv.APPDATA}`)
  console.log(`[e2e-tauri] LOCALAPPDATA: ${profileEnv.LOCALAPPDATA}`)

  appProcess = spawn(binary, [], {
    env: {
      ...process.env,
      ...pathEnv,
      ...profileEnv,
      TAURI_WEBDRIVER_PORT: String(webdriverPort),
      MAEKON_DISABLE_TRAY: '1',
      RUST_LOG: 'maekon=info,tauri_plugin_webdriver=debug',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  if (appProcess.pid) {
    console.log(`[e2e-tauri] App PID: ${appProcess.pid}`)
  }

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
  const currentProcess = appProcess
  appProcess = null
  console.log(`[e2e-tauri] Stopping app (PID: ${currentProcess.pid})`)

  const command = currentProcess.pid ? terminationCommand(currentProcess.pid) : null
  if (command) {
    try {
      execFileSync(command.file, command.args, { stdio: 'ignore' })
      return
    } catch {
      currentProcess.kill()
      return
    }
  }

  currentProcess.kill('SIGTERM')
  // Escalate after 3s if still alive
  setTimeout(() => {
    if (!currentProcess.killed) {
      console.log('[e2e-tauri] Escalating to SIGKILL')
      currentProcess.kill('SIGKILL')
    }
  }, 3000)
}
