import { describe, expect, it } from 'vitest'
import { sanitizeControlChars } from './sanitize'

// Build adversarial inputs from code points so the test source carries no
// embedded control bytes.
const RLO = String.fromCharCode(0x202e) // right-to-left override (bidi spoof)
const ZWSP = String.fromCharCode(0x200b) // zero-width space
const BOM = String.fromCharCode(0xfeff)
const NUL = String.fromCharCode(0x00)
const DEL = String.fromCharCode(0x7f)

describe('sanitizeControlChars', () => {
  it('passes ordinary text through unchanged', () => {
    expect(sanitizeControlChars('git status')).toBe('git status')
    expect(sanitizeControlChars('Coinbase — Buy/Sell')).toBe('Coinbase — Buy/Sell')
  })

  it('strips bidi-override / zero-width / control / DEL / BOM characters', () => {
    expect(sanitizeControlChars(`rm${RLO} -rf /`)).toBe('rm -rf /')
    expect(sanitizeControlChars(`pass${ZWSP}word`)).toBe('password')
    expect(sanitizeControlChars(`${BOM}git`)).toBe('git')
    expect(sanitizeControlChars(`a${NUL}b${DEL}c`)).toBe('abc')
  })

  it('strips bidi isolates (LRI/RLI/FSI/PDI) and the Arabic Letter Mark', () => {
    const lri = String.fromCharCode(0x2066) // left-to-right isolate
    const pdi = String.fromCharCode(0x2069) // pop directional isolate
    const alm = String.fromCharCode(0x061c) // Arabic letter mark
    expect(sanitizeControlChars(`a${lri}b${pdi}c${alm}d`)).toBe('abcd')
  })

  it('leaves no disguising character in the result', () => {
    const cleaned = sanitizeControlChars(`safe${RLO}${ZWSP}${NUL}command`)
    expect(cleaned).toBe('safecommand')
    for (const ch of cleaned) {
      expect(ch.codePointAt(0) ?? 0).toBeGreaterThan(0x1f)
    }
  })

  it('preserves multi-byte / emoji content', () => {
    expect(sanitizeControlChars('日本語 résumé 🚀')).toBe('日本語 résumé 🚀')
  })
})
