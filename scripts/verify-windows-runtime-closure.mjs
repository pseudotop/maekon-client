#!/usr/bin/env node

import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { basename, dirname, relative, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const SYSTEM_DLLS = new Set([
  'advapi32.dll',
  'bcrypt.dll',
  'bcryptprimitives.dll',
  'cfgmgr32.dll',
  'comctl32.dll',
  'combase.dll',
  'comdlg32.dll',
  'crypt32.dll',
  'd3d11.dll',
  'dbghelp.dll',
  'dcomp.dll',
  'dnsapi.dll',
  'dwmapi.dll',
  'dxgi.dll',
  'gdi32.dll',
  'gdi32full.dll',
  'imm32.dll',
  'iphlpapi.dll',
  'kernel32.dll',
  'kernelbase.dll',
  // Windows Multimedia Device API — the WASAPI enumerator cpal opens for audio
  // capture. Ships in %SystemRoot%\System32 on every supported Windows version
  // and is never redistributable, so it belongs with winmm.dll rather than in
  // the payload. It entered the import table when cpal moved to 0.18 (#7207,
  // 2026-06-30), after rc.6 was tagged, and the release lane had not run since.
  'mmdevapi.dll',
  'msimg32.dll',
  'ncrypt.dll',
  'netapi32.dll',
  'ntdll.dll',
  'ole32.dll',
  'oleaut32.dll',
  'pdh.dll',
  'powrprof.dll',
  'propsys.dll',
  'psapi.dll',
  'rpcrt4.dll',
  'secur32.dll',
  'setupapi.dll',
  'shell32.dll',
  'shlwapi.dll',
  'user32.dll',
  'userenv.dll',
  'uxtheme.dll',
  'version.dll',
  'winhttp.dll',
  'winmm.dll',
  'wintrust.dll',
  'ws2_32.dll',
  'wtsapi32.dll',
])

function normalizeRelative(root, path) {
  return relative(root, path).replaceAll('\\', '/')
}

function collectPayloadFiles(root) {
  const files = []
  const visit = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = resolve(directory, entry.name)
      if (entry.isDirectory()) visit(path)
      else if (entry.isFile()) files.push(path)
    }
  }
  visit(root)
  return files
}

function isWindowsSystemImport(name) {
  const lower = name.toLowerCase()
  return (
    lower.startsWith('api-ms-win-') ||
    lower.startsWith('ext-ms-win-') ||
    SYSTEM_DLLS.has(lower)
  )
}

function parseDumpbinDependents(stdout) {
  return [...stdout.matchAll(/^\s+([A-Za-z0-9_.+-]+\.dll)\s*$/gim)].map((match) => match[1])
}

function readImportsWithDumpbin(path, dependencyTool) {
  const result = spawnSync(dependencyTool, ['/nologo', '/dependents', path], {
    encoding: 'utf8',
    windowsHide: true,
  })
  if (result.error) throw new Error(`failed to start ${dependencyTool}: ${result.error.message}`)
  if (result.status !== 0) {
    throw new Error(`dumpbin failed for ${path}: ${(result.stderr || result.stdout).trim()}`)
  }
  return parseDumpbinDependents(result.stdout)
}

function resolveEntryBinary(root, files, requested) {
  const explicit = resolve(root, requested)
  if (existsSync(explicit)) return explicit
  const matches = files.filter((path) => basename(path).toLowerCase() === requested.toLowerCase())
  if (matches.length !== 1) {
    throw new Error(`${requested}: expected exactly one payload binary, found ${matches.length}`)
  }
  return matches[0]
}

export function verifyWindowsRuntimeClosure({
  payloadRoot,
  binaries = ['maekon.exe', 'maekon-sandbox-worker.exe'],
  dependencyTool = 'dumpbin.exe',
  dependencyMap = null,
}) {
  const root = resolve(payloadRoot)
  const files = collectPayloadFiles(root)
  const filesByName = new Map()
  for (const path of files) {
    const key = basename(path).toLowerCase()
    filesByName.set(key, [...(filesByName.get(key) ?? []), path])
  }

  const queue = binaries.map((binary) => resolveEntryBinary(root, files, binary))
  const visited = new Set()
  const bundledDependencies = new Set()
  const missing = []

  while (queue.length > 0) {
    const path = queue.shift()
    const relativePath = normalizeRelative(root, path)
    const visitKey = relativePath.toLowerCase()
    if (visited.has(visitKey)) continue
    visited.add(visitKey)

    let imports
    if (dependencyMap) {
      const mapKey = Object.keys(dependencyMap).find((key) => key.toLowerCase() === visitKey)
      if (!mapKey) throw new Error(`${relativePath}: fixture dependency map entry is required`)
      imports = dependencyMap[mapKey]
    } else {
      imports = readImportsWithDumpbin(path, dependencyTool)
    }

    if (!Array.isArray(imports) || imports.some((name) => typeof name !== 'string')) {
      throw new Error(`${relativePath}: dependency list must contain DLL names`)
    }

    for (const imported of imports) {
      const key = imported.toLowerCase()
      const sibling = resolve(dirname(path), imported)
      let bundledPath = existsSync(sibling) ? sibling : null
      if (!bundledPath) {
        const candidates = filesByName.get(key) ?? []
        if (candidates.length > 1) {
          missing.push(`${relativePath} -> ${imported} (ambiguous payload copies)`)
          continue
        }
        bundledPath = candidates[0] ?? null
      }

      if (bundledPath) {
        bundledDependencies.add(normalizeRelative(root, bundledPath))
        queue.push(bundledPath)
      } else if (!isWindowsSystemImport(imported)) {
        missing.push(`${relativePath} -> ${imported}`)
      }
    }
  }

  if (missing.length > 0) {
    throw new Error(`unresolved non-system DLL imports:\n${missing.sort().join('\n')}`)
  }

  return {
    schema_version: 'maekon.windows-runtime-closure.v1',
    result: 'pass',
    payload_root: root,
    entry_binaries: binaries,
    inspected_pe_count: visited.size,
    bundled_dependencies: [...bundledDependencies].sort(),
  }
}

function parseArgs(argv) {
  const options = { binaries: [] }
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--payload-root') options.payloadRoot = argv[++index]
    else if (argument === '--binary') options.binaries.push(argv[++index])
    else if (argument === '--dependency-tool') options.dependencyTool = argv[++index]
    else if (argument === '--fixture-dependency-map') options.fixtureMapPath = argv[++index]
    else if (argument === '--allow-fixture-dependency-map') options.allowFixtureMap = true
    else throw new Error(`unknown argument: ${argument}`)
  }
  if (!options.payloadRoot) throw new Error('--payload-root is required')
  if (options.binaries.length === 0) delete options.binaries
  if (options.fixtureMapPath && !options.allowFixtureMap) {
    throw new Error('--fixture-dependency-map requires --allow-fixture-dependency-map')
  }
  if (options.fixtureMapPath) {
    options.dependencyMap = JSON.parse(readFileSync(options.fixtureMapPath, 'utf8'))
  }
  delete options.fixtureMapPath
  delete options.allowFixtureMap
  return options
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    console.log(JSON.stringify(verifyWindowsRuntimeClosure(parseArgs(process.argv.slice(2))), null, 2))
  } catch (error) {
    console.error(`[windows-runtime-closure] ${error.message}`)
    process.exitCode = 1
  }
}
