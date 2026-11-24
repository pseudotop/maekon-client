/**
 * #13738 — Korean localization completeness guard.
 *
 * Same rule as the Japanese guard (#8096): a ko leaf whose value is a
 * byte-identical copy of English is almost always an untranslated string.
 * Allowed identical values are the shared baseline in `../identical-invariants.ts`
 * (brand names, acronyms, example URLs, interpolation-only strings) plus the
 * Korean-only entries in `KO_IDENTICAL_INVARIANTS`. Anything else fails, so a
 * new English copy in ko.json needs a translation or an allowlist entry with a
 * rationale.
 */

import { describe, expect, it } from 'vitest'
import { INVARIANT_KEYS, KO_IDENTICAL_INVARIANTS, KO_INVARIANT_KEYS } from '../identical-invariants'
import en from '../locales/en.json'
import ko from '../locales/ko.json'

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

const flatEn = flatten(en as JsonRecord)
const flatKo = flatten(ko as JsonRecord)

/** Leaf keys whose ko value is a byte-identical string copy of en. */
function identicalStringLeaves(): string[] {
  return Object.keys(flatEn)
    .filter((k) => typeof flatEn[k] === 'string' && flatEn[k] === flatKo[k])
    .sort()
}

describe('ko localization completeness (#13738)', () => {
  it('no ko leaf is accidental English fallback (identical value ⇒ must be allowlisted)', () => {
    const unexpected = identicalStringLeaves().filter((k) => !INVARIANT_KEYS.has(k) && !KO_INVARIANT_KEYS.has(k))
    // Translate the listed ko key(s). Only a proper noun or a string with
    // nothing to translate goes into `identical-invariants.ts`, with a rationale.
    expect(unexpected, 'untranslated ko fallback leaves').toEqual([])
  })

  it('Korean-only allowlist has no stale entries (each key still exists and is still identical)', () => {
    const stale = [...KO_INVARIANT_KEYS].filter((k) => !(k in flatEn) || flatEn[k] !== flatKo[k]).sort()
    expect(stale, 'stale Korean-only allowlist entries').toEqual([])
  })

  it('Korean-only entries are unique, are not already shared, and carry a rationale', () => {
    const keys = KO_IDENTICAL_INVARIANTS.map((entry) => entry.key)
    expect(new Set(keys).size, 'duplicate Korean-only keys').toBe(keys.length)
    expect(
      keys.filter((k) => INVARIANT_KEYS.has(k)),
      'already in the shared baseline',
    ).toEqual([])
    for (const entry of KO_IDENTICAL_INVARIANTS) {
      expect(entry.rationale.trim().length, `rationale for ${entry.key}`).toBeGreaterThan(0)
    }
  })

  it('still sees identical values, so an empty result means translated rather than unread', () => {
    // Positive control: the brand name and the acronyms are identical on purpose.
    expect(identicalStringLeaves()).toEqual(expect.arrayContaining(['trayDashboard.title', 'reauth.pinLabel']))
  })
})
