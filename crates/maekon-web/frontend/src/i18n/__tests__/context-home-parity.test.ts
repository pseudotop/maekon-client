/**
 * #9611 WD-02.3 — `/home` context home i18n 5-locale parity.
 *
 * A locale missing one of these keys renders the raw dotted key on the screen
 * the investor demo lands on after sign-in, so the key set is asserted
 * identical everywhere rather than only in en.
 *
 * `nav.contextHome` is checked in the same loop: it is the CommandPalette label
 * for the route, which is what makes `/home` reachable by name rather than only
 * as a post-login redirect target.
 *
 * The interpolation placeholders are asserted too. A translation that drops
 * `{{count}}` does not fail to render — it renders a sentence with the number
 * silently missing, which is worse than a visible broken key.
 */

import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const

type JsonRecord = Record<string, unknown>

function contextHomeBlock(locale: JsonRecord): JsonRecord {
  const block = locale.contextHome as JsonRecord | undefined
  if (!block) throw new Error('missing top-level contextHome block')
  return block
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

function lookup(block: JsonRecord, dotted: string): unknown {
  return dotted.split('.').reduce<unknown>((acc, part) => (acc as JsonRecord | undefined)?.[part], block)
}

/** Every `contextHome.*` key `pages/home/ContextHomePage.tsx` resolves. */
const REQUIRED = [
  'actorLine',
  'announce.bridgeAbsent',
  'announce.complete',
  'announce.denied',
  'announce.empty',
  'announce.malformed',
  'announce.partial',
  'announce.reauth',
  'announce.refreshing',
  'announce.unavailable',
  'bridgeAbsent.body',
  'denied.body',
  'empty.projects',
  'empty.threads',
  'loading',
  'malformed.body',
  'moreParticipants',
  'moreRows',
  'participant.external',
  'project.counterparty',
  'project.role',
  'reauth.body',
  'refresh',
  'retry',
  'section.backendUnavailable',
  'section.timeout',
  'section.unavailableFlag',
  'sections.mail',
  'sections.messenger',
  'sections.projects',
  'stale.badge',
  'syntheticBadge.description',
  'syntheticBadge.label',
  'title',
  'unavailable.body',
]

/** Keys whose copy interpolates a value the page supplies. */
const PLACEHOLDERS: Record<string, string[]> = {
  actorLine: ['{{actor}}', '{{org}}'],
  moreRows: ['{{count}}'],
  moreParticipants: ['{{count}}'],
  'participant.external': ['{{name}}'],
  'project.role': ['{{role}}'],
  'project.counterparty': ['{{name}}'],
}

describe('context home i18n parity (5 locales)', () => {
  const reference = flatKeys(contextHomeBlock(en as JsonRecord))

  it('en baseline carries every key the page renders', () => {
    expect(reference).toEqual(expect.arrayContaining(REQUIRED))
  })

  for (const [name, locale] of Object.entries(LOCALES)) {
    const block = contextHomeBlock(locale as JsonRecord)

    it(`${name} has the same contextHome key set as en`, () => {
      expect(flatKeys(block)).toEqual(reference)
    })

    it(`${name} leaves no context home string empty`, () => {
      for (const key of REQUIRED) {
        const value = lookup(block, key)
        expect(typeof value, `${name}.${key}`).toBe('string')
        expect((value as string).trim(), `${name}.${key}`).not.toBe('')
      }
    })

    it(`${name} keeps every interpolation placeholder`, () => {
      for (const [key, tokens] of Object.entries(PLACEHOLDERS)) {
        const value = lookup(block, key) as string
        for (const token of tokens) {
          expect(value, `${name}.${key} must keep ${token}`).toContain(token)
        }
      }
    })

    it(`${name} labels the /home route in the command palette`, () => {
      const nav = (locale as JsonRecord).nav as JsonRecord
      expect(typeof nav.contextHome).toBe('string')
      expect((nav.contextHome as string).trim()).not.toBe('')
    })

    it(`${name} names the post-login destination on the sign-in screen`, () => {
      // #9611 renamed `login.continueToDashboard` with its destination.
      const login = (locale as JsonRecord).login as JsonRecord
      expect(typeof login.continueToHome).toBe('string')
      expect(login.continueToDashboard).toBeUndefined()
    })
  }
})
