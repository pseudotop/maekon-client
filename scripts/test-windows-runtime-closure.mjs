#!/usr/bin/env node

import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { verifyWindowsRuntimeClosure } from './verify-windows-runtime-closure.mjs'

function withPayload(files, dependencyMap, callback) {
  const root = mkdtempSync(resolve(tmpdir(), 'maekon-runtime-closure-'))
  try {
    for (const file of files) {
      const path = resolve(root, file)
      mkdirSync(resolve(path, '..'), { recursive: true })
      writeFileSync(path, 'PE fixture\n')
    }
    callback(() => verifyWindowsRuntimeClosure({ payloadRoot: root, dependencyMap }))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

withPayload(
  ['maekon.exe', 'maekon-sandbox-worker.exe'],
  {
    'maekon.exe': ['KERNEL32.dll', 'libcrypto-3-x64.dll'],
    'maekon-sandbox-worker.exe': ['KERNEL32.dll'],
  },
  (verify) => assert.throws(verify, /maekon\.exe -> libcrypto-3-x64\.dll/),
)

withPayload(
  ['maekon.exe', 'maekon-sandbox-worker.exe', 'libcrypto-3-x64.dll'],
  {
    'maekon.exe': ['KERNEL32.dll', 'libcrypto-3-x64.dll'],
    'maekon-sandbox-worker.exe': ['USER32.dll'],
    'libcrypto-3-x64.dll': ['KERNEL32.dll', 'BCRYPT.dll'],
  },
  (verify) => {
    const result = verify()
    assert.equal(result.result, 'pass')
    assert.deepEqual(result.bundled_dependencies, ['libcrypto-3-x64.dll'])
    assert.equal(result.inspected_pe_count, 3)
  },
)

withPayload(
  ['maekon.exe', 'maekon-sandbox-worker.exe'],
  {
    'maekon.exe': ['api-ms-win-core-file-l1-2-0.dll', 'KERNEL32.dll'],
    'maekon-sandbox-worker.exe': ['VCRUNTIME140.dll'],
  },
  (verify) => assert.throws(verify, /maekon-sandbox-worker\.exe -> VCRUNTIME140\.dll/),
)

withPayload(
  [
    'maekon.exe',
    'maekon-sandbox-worker.exe',
    'vcruntime140.dll',
    'vcruntime140_1.dll',
  ],
  {
    'maekon.exe': ['KERNEL32.dll', 'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll'],
    'maekon-sandbox-worker.exe': ['KERNEL32.dll'],
    'vcruntime140.dll': ['KERNEL32.dll', 'api-ms-win-crt-runtime-l1-1-0.dll'],
    'vcruntime140_1.dll': ['KERNEL32.dll', 'VCRUNTIME140.dll'],
  },
  (verify) => {
    const result = verify()
    assert.equal(result.result, 'pass')
    assert.deepEqual(result.bundled_dependencies, [
      'VCRUNTIME140.dll',
      'VCRUNTIME140_1.dll',
    ])
  },
)

withPayload(
  ['maekon.exe', 'maekon-sandbox-worker.exe', 'nested/libcrypto-3-x64.dll'],
  {
    'maekon.exe': ['libcrypto-3-x64.dll'],
    'maekon-sandbox-worker.exe': [],
  },
  (verify) => assert.throws(verify, /fixture dependency map entry is required/),
)

console.log('[test-windows-runtime-closure] ok')
