/**
 * #9603 WD-02.1 — `/login` screen i18n 5-locale parity.
 *
 * The screen's own chrome lives in the top-level `login.*` namespace (the form
 * inside it reuses `settings.account.login.*`, whose parity is already pinned by
 * `tauriIpcErrors.test.ts` because those keys are also the IpcError→UI mapping
 * targets). A locale missing one of these keys renders the raw dotted key on a
 * screen the investor demo opens on, so the key set is asserted identical
 * everywhere rather than only in en.
 *
 * `nav.signIn` is checked in the same loop: it is the CommandPalette label for
 * the route, which is what makes `/login` reachable by name rather than only by
 * deep link.
 */

import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const

type JsonRecord = Record<string, unknown>

function loginBlock(locale: JsonRecord): JsonRecord {
  const login = locale.login as JsonRecord | undefined
  if (!login) throw new Error('missing top-level login block')
  return login
}

function flatKeys(obj: JsonRecord, prefix = ''): string[] {
  const out: string[] = []
  for (const [k, v] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${k}` : k
    if (v !== null && typeof v === 'object' && !Array.isArray(v)) {
      out.push(...flatKeys(v as JsonRecord, path))
    } else {
      out.push(path)
    }
  }
  return out.sort()
}

/** Every `login.*` key `pages/auth/LoginPage.tsx` resolves. */
const REQUIRED = [
  // #9611 WD-02.3: renamed with its destination — a signed-in user now
  // continues to `/home` (the context home), not to the local-first dashboard.
  'continueToHome',
  'continueWithoutSignIn',
  'localFirstNote',
  'pageSubtitle',
  'pageTitle',
  'signedInTitle',
  'unavailableTitle',
]

describe('login screen i18n parity (5 locales)', () => {
  const reference = flatKeys(loginBlock(en as JsonRecord))

  it('en baseline carries every key the page renders', () => {
    expect(reference).toEqual(expect.arrayContaining(REQUIRED))
  })

  for (const [name, locale] of Object.entries(LOCALES)) {
    it(`${name} has the same login key set as en`, () => {
      expect(flatKeys(loginBlock(locale as JsonRecord))).toEqual(reference)
    })

    it(`${name} labels the /login route in the command palette`, () => {
      const nav = (locale as JsonRecord).nav as JsonRecord
      expect(typeof nav.signIn).toBe('string')
      expect(nav.signIn).not.toBe('')
    })
  }
})
