/**
 * #8096 — Japanese localization completeness guard.
 *
 * Key parity between en and ja is already asserted by `translateError.test.ts`.
 * This test goes further: it flags *value* fallback — ja leaves whose value is
 * byte-identical to English — which is almost always an untranslated string.
 * The narrow set of intentionally-identical terms (brand/protocol/acronym/URL/
 * endonym) lives in `../identical-invariants.ts`; anything else that is
 * identical fails here, forcing a decision (translate it, or allowlist it with
 * a rationale).
 *
 * Scope: ja only, for now. ko/zh-CN/es also have identical-value leaves, but at
 * much larger counts measured on this HEAD (ko 64, zh-CN 296, es 337 vs ja's
 * post-fix 22 invariants). Translating those is out of scope for #8096; a
 * follow-up should translate each locale and then widen this guard to it,
 * reusing `IDENTICAL_INVARIANTS` as the shared cross-locale invariant baseline.
 * TODO(#8096-followup): extend the identical-value guard to ko, zh-CN, es.
 */

import { describe, expect, it } from 'vitest'
import { IDENTICAL_INVARIANTS, INVARIANT_KEYS } from '../identical-invariants'
import en from '../locales/en.json'
import ja from '../locales/ja.json'

type JsonRecord = Record<string, unknown>

/** Flattens a nested object into a map of dot-separated leaf path -> value. */
function flatten(obj: JsonRecord, prefix = '', out: Record<string, unknown> = {}): Record<string, unknown> {
  for (const [k, v] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${k}` : k
    if (v !== null && typeof v === 'object' && !Array.isArray(v)) {
      flatten(v as JsonRecord, path, out)
    } else {
      out[path] = v
    }
  }
  return out
}

/** Leaf keys whose ja value is a byte-identical string copy of en. */
function identicalStringLeaves(): string[] {
  const flatEn = flatten(en as JsonRecord)
  const flatJa = flatten(ja as JsonRecord)
  return Object.keys(flatEn)
    .filter((k) => typeof flatEn[k] === 'string' && flatEn[k] === flatJa[k])
    .sort()
}

describe('ja localization completeness (#8096)', () => {
  it('no ja leaf is accidental English fallback (identical value ⇒ must be allowlisted)', () => {
    const identical = identicalStringLeaves()
    const unexpected = identical.filter((k) => !INVARIANT_KEYS.has(k))
    // If this fails, either translate the listed ja key(s), or — only for a
    // genuine brand/protocol/acronym/URL/endonym single token — add it to
    // `identical-invariants.ts` with a one-line rationale.
    expect(unexpected, 'untranslated ja fallback leaves').toEqual([])
  })

  it('allowlist has no stale entries (every allowlisted key still exists and is still identical)', () => {
    const flatEn = flatten(en as JsonRecord)
    const flatJa = flatten(ja as JsonRecord)
    const stale = [...INVARIANT_KEYS].filter((k) => !(k in flatEn) || flatEn[k] !== flatJa[k]).sort()
    // A key that was translated (or renamed/removed by another change) should be
    // dropped from the allowlist so the list stays an honest inventory.
    expect(stale, 'stale allowlist entries').toEqual([])
  })

  it('allowlist entries are unique and each carries a non-empty rationale', () => {
    const keys = IDENTICAL_INVARIANTS.map((e) => e.key)
    expect(new Set(keys).size, 'duplicate allowlist keys').toBe(keys.length)
    for (const entry of IDENTICAL_INVARIANTS) {
      expect(entry.rationale.trim().length, `rationale for ${entry.key}`).toBeGreaterThan(0)
    }
  })

  it('the allowlist exactly covers the current identical-value set (no over/under-allow)', () => {
    // Pins the fixed count so a future accidental fallback OR an over-broad
    // allowlist entry is caught precisely, not just directionally.
    expect(identicalStringLeaves()).toEqual([...INVARIANT_KEYS].sort())
  })
})
