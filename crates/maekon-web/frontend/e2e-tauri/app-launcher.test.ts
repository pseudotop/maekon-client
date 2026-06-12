import { normalize } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  binaryCandidates,
  e2eProfileEnv,
  runtimePathEnv,
  staleProcessCleanupCommand,
  terminationCommand,
} from './app-launcher'

describe('e2e-tauri app launcher platform helpers', () => {
  it('looks for maekon.exe on Windows', () => {
    const candidates = binaryCandidates('C:\\repo\\clients\\maekon-client', 'win32').map((candidate) =>
      normalize(candidate),
    )

    expect(candidates).toEqual([
      normalize('C:\\repo\\clients\\maekon-client\\target\\debug\\maekon.exe'),
      normalize('C:\\repo\\clients\\maekon-client\\target\\release\\maekon.exe'),
    ])
  })

  it('prefers an explicit Cargo target directory for Windows launch artifacts', () => {
    const candidates = binaryCandidates(
      'C:\\repo\\clients\\maekon-client',
      'win32',
      'C:\\codex-cache\\cargo-target\\maekon-client-msvc-launch',
    ).map((candidate) => normalize(candidate))

    expect(candidates).toEqual([
      normalize('C:\\codex-cache\\cargo-target\\maekon-client-msvc-launch\\debug\\maekon.exe'),
      normalize('C:\\codex-cache\\cargo-target\\maekon-client-msvc-launch\\release\\maekon.exe'),
      normalize('C:\\repo\\clients\\maekon-client\\target\\debug\\maekon.exe'),
      normalize('C:\\repo\\clients\\maekon-client\\target\\release\\maekon.exe'),
    ])
  })

  it('sets isolated Windows app profile directories for e2e launches', () => {
    const env = e2eProfileEnv('C:\\repo\\clients\\maekon-client', 'C:\\evidence\\profile')

    expect(normalize(env.APPDATA)).toBe(normalize('C:\\evidence\\profile\\roaming'))
    expect(normalize(env.LOCALAPPDATA)).toBe(normalize('C:\\evidence\\profile\\local'))
    expect(env.MAEKON_APP_FLAVOR).toBe('e2e')
  })

  it('adds the OpenSSL runtime bin directory to Windows launch PATH when configured', () => {
    const env = runtimePathEnv('win32', {
      Path: 'C:\\Windows\\System32',
      OPENSSL_DIR: 'C:\\Users\\dev\\scoop\\apps\\openssl\\current',
    })

    expect(env.Path).toBe('C:\\Users\\dev\\scoop\\apps\\openssl\\current\\bin;C:\\Windows\\System32')
  })

  it('uses a path-scoped Windows process cleanup command', () => {
    const command = staleProcessCleanupCommand('win32')

    expect(command.file).toBe('powershell.exe')
    expect(command.args.join(' ')).toContain('Win32_Process')
    expect(command.args.join(' ')).toContain('ExecutablePath')
    expect(command.args.join(' ')).toContain('target\\\\(debug|release)\\\\maekon\\.exe')
    expect(command.args.join(' ')).toContain('Stop-Process')
  })

  it('uses taskkill for Windows process-tree termination', () => {
    expect(terminationCommand(4242, 'win32')).toEqual({
      file: 'taskkill.exe',
      args: ['/PID', '4242', '/T', '/F'],
    })
  })
})
