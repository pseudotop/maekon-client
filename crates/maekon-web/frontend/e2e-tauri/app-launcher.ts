/**
 * Tauri app launcher for WebdriverIO E2E tests.
 *
 * Resolves the test binary and prepares its isolated environment.
 * @wdio/tauri-service owns process and WebDriver lifecycle.
 */
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync } from 'node:fs'
import { delimiter, dirname, posix as posixPath, resolve, win32 as winPath } from 'node:path'
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
  MAEKON_OFFLINE_MODE: string
}

type RuntimePathEnv = {
  Path?: string
  PATH?: string
}

function workspaceRoot(): string {
  return resolve(__dirname, '../../../..')
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

export function killStaleProcesses(): void {
  const { file, args } = staleProcessCleanupCommand()
  try {
    execFileSync(file, args, { stdio: 'ignore' })
  } catch {
    // Ignore — no stale processes
  }
}

/** Path API faithful to the simulated platform (host-independent tests, #8685). */
function pathFor(platform: NodeJS.Platform): typeof winPath | typeof posixPath {
  return platform === 'win32' ? winPath : posixPath
}

/** Return platform-specific candidate binary paths. */
export function binaryCandidates(
  root: string,
  platform: NodeJS.Platform = process.platform,
  cargoTargetDir = process.env.CARGO_TARGET_DIR,
): string[] {
  const p = pathFor(platform)
  const extension = platform === 'win32' ? '.exe' : ''
  const profilePaths = [`debug/maekon${extension}`, `release/maekon${extension}`]
  const explicitTargetCandidates = cargoTargetDir
    ? profilePaths.map((profilePath) => p.resolve(cargoTargetDir, profilePath))
    : []
  const workspaceTargetCandidates = profilePaths.map((profilePath) => p.resolve(root, 'target', profilePath))

  return [...explicitTargetCandidates, ...workspaceTargetCandidates]
}

/** Return isolated app profile directories for E2E launches. */
export function e2eProfileEnv(
  root: string,
  profileRoot = process.env.MAEKON_E2E_PROFILE_ROOT,
  platform: NodeJS.Platform = process.platform,
): E2eProfileEnv {
  const p = pathFor(platform)
  const base = profileRoot ? p.resolve(root, profileRoot) : p.resolve(root, '.evidence', 'e2e-tauri', 'profile')

  return {
    APPDATA: p.resolve(base, 'roaming'),
    LOCALAPPDATA: p.resolve(base, 'local'),
    MAEKON_APP_FLAVOR: process.env.MAEKON_APP_FLAVOR ?? 'e2e',
    // Desktop E2E must never depend on, or send traffic to, a real account or
    // server. Callers can opt into a synthetic server explicitly.
    MAEKON_OFFLINE_MODE: process.env.MAEKON_E2E_OFFLINE_MODE ?? '1',
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
export function findBinary(): string {
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

/** Prepare the service-owned app process environment. */
export function prepareE2eEnvironment(): Record<string, string> {
  const profileEnv = e2eProfileEnv(workspaceRoot())
  ensureProfileDirs(profileEnv)
  return {
    ...profileEnv,
    ...runtimePathEnv(),
    MAEKON_DISABLE_TRAY: '1',
    RUST_LOG: 'maekon=info,tauri_plugin_wdio=info,tauri_plugin_wdio_webdriver=info',
  }
}

/** Return a Windows process-tree termination command for diagnostics. */
export function terminationCommand(pid: number, platform: NodeJS.Platform = process.platform): CommandSpec | null {
  if (platform !== 'win32') return null
  return {
    file: 'taskkill.exe',
    args: ['/PID', String(pid), '/T', '/F'],
  }
}
