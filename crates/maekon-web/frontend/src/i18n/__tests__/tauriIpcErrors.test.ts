/**
 * #9492 item 4 — out-of-registry Tauri IpcError code coverage.
 *
 * `wire-errors.*.json` is locked key-for-key to the 54-code ADR-019 registry
 * (`translateError.test.ts`), so codes minted with `IpcError::new(..)` inside
 * `src-tauri/src/commands/*.rs` cannot live there. They are localized through
 * UI-namespace keys instead, and this suite is what keeps that table honest:
 * every mapped key must resolve in all five locale files, and the mapped set
 * must stay exactly the set of out-of-registry codes the Rust side mints.
 */

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'
import {
  describeIpcError,
  LOGIN_FORM_IPC_ERROR_KEYS,
  loginFormIpcErrorKey,
  TAURI_IPC_ERROR_KEYS,
  wireLocaleFor,
} from '../tauriIpcErrors'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const

const __dirname = path.dirname(fileURLToPath(import.meta.url))

/**
 * The canonical ADR-019 wire-code registry, read from the Rust snapshot fixture
 * rather than restated here (same source `translateError.test.ts` reads).
 */
function readWireCodeRegistry(): string[] {
  const registryPath = path.resolve(
    __dirname,
    '../../../../../../crates/maekon-core/tests/wire_contract_snapshot.expected.txt',
  )
  return fs
    .readFileSync(registryPath, 'utf-8')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
}

/**
 * Every UI key any lookup in this module can return — the plain table plus the
 * sign-in-form-only sentences, which belong to no other parity loop.
 */
const ALL_MAPPED_KEYS = [...Object.values(TAURI_IPC_ERROR_KEYS), ...Object.values(LOGIN_FORM_IPC_ERROR_KEYS)]

type JsonRecord = Record<string, unknown>

/** Resolve a dot-separated i18n key against a locale document. */
function lookup(locale: JsonRecord, key: string): unknown {
  return key.split('.').reduce<unknown>((node, part) => {
    if (node !== null && typeof node === 'object' && part in (node as JsonRecord)) {
      return (node as JsonRecord)[part]
    }
    return undefined
  }, locale)
}

/**
 * The complete set of `IpcError::new(..)` codes minted in `src-tauri` that are
 * absent from `crates/maekon-core/tests/wire_contract_snapshot.expected.txt`.
 * Enumerated with `grep -rn 'IpcError::new(' src-tauri/src/`; the other nine
 * literals reuse registry codes and are covered by `translateError`. The
 * disjointness half of that claim is checked against the real fixture below.
 */
const OUT_OF_REGISTRY_CODES = [
  'auth.feature_disabled',
  'auth.invalid_arguments',
  'auth.token_manager_unavailable',
  'handoff.launch_failed',
  'handoff.no_handler',
  'handoff.rejected',
  'storage.unavailable',
]

describe('Tauri-layer IpcError → UI key mapping (#9492)', () => {
  it('maps exactly the out-of-registry codes the Rust side mints', () => {
    // An extra entry means a registry code was shadowed (its catalog template
    // would stop being used); a missing one means that code fell back to its
    // raw Rust English literal in the UI — the #9492 item 4 defect.
    expect(Object.keys(TAURI_IPC_ERROR_KEYS).sort()).toEqual(OUT_OF_REGISTRY_CODES)
  })

  it('shadows no ADR-019 registry code (checked against the Rust snapshot, not a restated list)', () => {
    // The list above is a declaration; on its own it proves nothing about the
    // registry it claims to be disjoint from. Read the real fixture and assert
    // the disjointness directly, so adding a code that DOES have a catalog
    // template — which would silently replace its `{message}` template with a
    // fixed UI sentence — fails here.
    const registry = readWireCodeRegistry()
    expect(registry).toHaveLength(55)

    const shadowed = Object.keys(TAURI_IPC_ERROR_KEYS).filter((code) => registry.includes(code))
    expect(shadowed, 'mapped codes that already have a wire-catalog template').toEqual([])
  })

  for (const [name, locale] of Object.entries(LOCALES)) {
    it(`${name} defines every mapped key`, () => {
      for (const key of ALL_MAPPED_KEYS) {
        const value = lookup(locale as JsonRecord, key)
        expect(typeof value, `${name}: ${key}`).toBe('string')
        expect((value as string).length, `${name}: ${key}`).toBeGreaterThan(0)
      }
    })
  }

  it('storage.unavailable renders the localized sentence, never the Rust literal', () => {
    // The literal `commands/consent.rs::erase_all_local_data` returns when GDPR
    // Art. 17 frame erasure cannot be verified.
    const rustMessage = 'frame storage unavailable — cannot verify GDPR frame erasure completed'
    const translated = describeIpcError(
      { code: 'storage.unavailable', message: rustMessage },
      (key) => `t:${key}`,
      'en',
    )

    expect(translated).toBe('t:errors.ipc.storageUnavailable')
    expect(translated).not.toContain('frame storage unavailable')
  })

  it('never renders the Rust-side handoff detail (#9707)', () => {
    // `handoff.rejected`'s message names the rule that refused the target —
    // useful in a log, meaningless to a user, and English-only. Same for the
    // raw OS exit status in `handoff.launch_failed`. The Rust side already
    // keeps the rejected URL out of these strings; this keeps the strings
    // themselves out of the UI.
    const cases = [
      ['handoff.rejected', 'mailto domain `gmail.com` is not an RFC 2606/6761 reserved domain'],
      ['handoff.launch_failed', '/usr/bin/open exited with exit status: 1'],
      ['handoff.no_handler', 'no application is registered to open this target'],
    ] as const

    for (const [code, rustMessage] of cases) {
      const translated = describeIpcError({ code, message: rustMessage }, (key) => `t:${key}`, 'ko')
      expect(translated).toBe(`t:${TAURI_IPC_ERROR_KEYS[code]}`)
      expect(translated).not.toContain(rustMessage)
    }
  })

  it('distinguishes "no mail app" from "handoff failed" (#9627 recovery branches)', () => {
    // #9627 has to define separate recovery for a missing mail app and a failed
    // handoff. Mapping both to one sentence would make one of those branches
    // unreachable in the UI regardless of what Rust returned.
    expect(TAURI_IPC_ERROR_KEYS['handoff.no_handler']).not.toBe(TAURI_IPC_ERROR_KEYS['handoff.launch_failed'])
    for (const locale of Object.values(LOCALES)) {
      const noHandler = lookup(locale as JsonRecord, TAURI_IPC_ERROR_KEYS['handoff.no_handler'])
      const launchFailed = lookup(locale as JsonRecord, TAURI_IPC_ERROR_KEYS['handoff.launch_failed'])
      expect(noHandler).not.toBe(launchFailed)
    }
  })

  it('falls back to translateError for registry codes and plain rejections', () => {
    // `config.invalid` IS in the registry, so it must keep using the wire
    // catalog template rather than being shadowed by this table.
    const registryCode = describeIpcError({ code: 'config.invalid', message: 'bad toml' }, (key) => `t:${key}`, 'en')
    expect(registryCode).not.toContain('t:')
    expect(registryCode).toContain('bad toml')

    // Non-IpcError rejections are untouched by the mapping.
    expect(describeIpcError(new Error('plain failure'), (key) => `t:${key}`, 'en')).toBe('plain failure')
  })

  it('classifies a 401 login rejection off the real two-layer message shape', () => {
    // Regression guard for the first #9492 attempt, where the matcher was
    // anchored at `login failure` and could never fire: `IpcError::from` stores
    // `CoreError::Auth`'s Display, so `login_with_org`'s own format arrives
    // nested inside `Authentication error [auth.failed]: `. Pinned Rust-side by
    // `commands::auth::tests::ipc_error_message_shape_matches_frontend_401_matcher`.
    const wireMessage =
      'Authentication error [auth.failed]: login failure (401 Unauthorized): ' +
      '{"type":"about:blank","title":"Unauthorized","status":401}'

    expect(loginFormIpcErrorKey({ code: 'auth.failed', message: wireMessage })).toBe(
      LOGIN_FORM_IPC_ERROR_KEYS.invalidCredentials,
    )
  })

  it('does not claim "wrong credentials" for the other auth.failed causes', () => {
    // `login_with_org` mints `auth.failed` for transport failures, 403s, 5xx,
    // and token-parse failures too — none of which are a credential problem.
    const others = [
      'Authentication error [auth.failed]: login request failure: error sending request for url (http://127.0.0.1:8000/api/v1/auth/tokens)',
      'Authentication error [auth.failed]: login failure (403 Forbidden): {"status":403}',
      'Authentication error [auth.failed]: login failure (500 Internal Server Error): {"status":500}',
      'Authentication error [auth.failed]: Token parsing failed: expected value at line 1 column 1',
    ]
    for (const message of others) {
      expect(loginFormIpcErrorKey({ code: 'auth.failed', message }), message).toBe(LOGIN_FORM_IPC_ERROR_KEYS.rejected)
    }
  })

  it('cannot be tricked into the 401 sentence by a response body that echoes the marker', () => {
    // The pattern stays anchored precisely so a server-controlled body cannot
    // smuggle the marker in from the tail of a non-401 failure.
    const spoofed =
      'Authentication error [auth.failed]: login failure (500 Internal Server Error): ' +
      '{"detail":"login failure (401 Unauthorized): nice try"}'

    expect(loginFormIpcErrorKey({ code: 'auth.failed', message: spoofed })).toBe(LOGIN_FORM_IPC_ERROR_KEYS.rejected)
  })

  it('leaves non-auth.failed codes on the shared table', () => {
    expect(loginFormIpcErrorKey({ code: 'auth.invalid_arguments', message: 'whatever' })).toBe(
      TAURI_IPC_ERROR_KEYS['auth.invalid_arguments'],
    )
    // A code in neither table falls through to translateError.
    expect(loginFormIpcErrorKey({ code: 'config.invalid', message: 'bad toml' })).toBeUndefined()
  })

  it('resolves the wire-error locale the same way every other surface does', () => {
    expect(wireLocaleFor('ko')).toBe('ko')
    expect(wireLocaleFor('ko-KR')).toBe('ko')
    expect(wireLocaleFor('ja')).toBe('en')
    expect(wireLocaleFor(undefined)).toBe('en')
  })
})
