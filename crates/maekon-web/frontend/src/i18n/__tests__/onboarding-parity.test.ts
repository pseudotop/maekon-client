/**
 * #13734 — first-run onboarding copy, 5-locale parity.
 *
 * Onboarding is the first screen a new user reads, so a missing key shows a
 * raw dotted path before anything else works. Every locale carries the same
 * non-empty keys with the same placeholders, every key the page asks for
 * exists, and no removed step leaves its copy behind.
 */

import { readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { IDENTICAL_INVARIANTS } from '../identical-invariants'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

type JsonRecord = Record<string, unknown>

const LOCALES: Record<string, JsonRecord> = { en, ko, ja, 'zh-CN': zhCN, es }

const SRC_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const ONBOARDING_SOURCE = readFileSync(join(SRC_ROOT, 'pages/Onboarding.tsx'), 'utf8')

/** Keys the page builds from a template, with every value the template takes. */
const TEMPLATE_KEYS: ReadonlyArray<{ template: string; keys: readonly string[] }> = [
  {
    template: `onboarding.intro.\${key}Title`,
    keys: ['intro.captureTitle', 'intro.resumeTitle', 'intro.searchTitle'],
  },
  {
    template: `onboarding.intro.\${key}Desc`,
    keys: ['intro.captureDesc', 'intro.resumeDesc', 'intro.searchDesc'],
  },
  {
    template: `onboarding.ai.features.\${copyKey}`,
    keys: ['ai.features.suggestions', 'ai.features.segmentSummary', 'ai.features.dailySummary'],
  },
  {
    template: `onboarding.ai.featureStatus.\${status}`,
    keys: [
      'ai.featureStatus.on',
      'ai.featureStatus.needsConsent',
      'ai.featureStatus.afterRestart',
      'ai.featureStatus.inSettings',
    ],
  },
  {
    template: `onboarding.ready.ai.\${aiStatus}`,
    keys: ['ready.ai.connected', 'ready.ai.restartPending', 'ready.ai.off'],
  },
]

/** Copy owned by the autostart prompt, which shares the namespace but not this page. */
const AUTOSTART_PREFIX = 'autostart.'

function flatEntries(obj: JsonRecord, prefix = ''): Array<[string, unknown]> {
  return Object.entries(obj).flatMap(([key, value]) => {
    const path = prefix ? `${prefix}.${key}` : key
    return value !== null && typeof value === 'object' && !Array.isArray(value)
      ? flatEntries(value as JsonRecord, path)
      : [[path, value] as [string, unknown]]
  })
}

function onboardingBlock(locale: JsonRecord): Map<string, unknown> {
  const block = locale.onboarding as JsonRecord | undefined
  if (!block) throw new Error('missing onboarding block')
  return new Map(flatEntries(block))
}

function placeholders(value: unknown): string[] {
  return [...String(value).matchAll(/\{\{\s*(\w+)\s*\}\}/g)].map((match) => match[1]).sort()
}

/** `onboarding.*` keys named as plain string literals in a source file. */
function literalKeys(source: string): string[] {
  return [...source.matchAll(/['"]onboarding\.([A-Za-z0-9_.]+)['"]/g)].map((match) => match[1])
}

function productSources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) {
      return name === '__tests__' || name === 'locales' ? [] : productSources(path)
    }
    return /\.tsx?$/.test(name) && !/\.(test|stories)\.tsx?$/.test(name) ? [path] : []
  })
}

const english = onboardingBlock(en)

describe('Onboarding localization (#13734)', () => {
  it('keeps the same non-empty keys in every supported locale', () => {
    const expected = [...english.keys()].sort()
    for (const [name, locale] of Object.entries(LOCALES)) {
      const block = onboardingBlock(locale)
      expect([...block.keys()].sort(), name).toEqual(expected)
      for (const [key, value] of block) {
        expect(typeof value === 'string' && value.trim() !== '', `${name}: ${key}`).toBe(true)
      }
    }
  })

  it('keeps the same placeholders in every locale', () => {
    for (const [name, locale] of Object.entries(LOCALES)) {
      for (const [key, value] of onboardingBlock(locale)) {
        expect(placeholders(value), `${name}: ${key}`).toEqual(placeholders(english.get(key)))
      }
    }
  })

  it('translates the page instead of falling back to English', () => {
    const invariant = new Set(IDENTICAL_INVARIANTS.map(({ key }) => key))
    for (const [name, locale] of Object.entries(LOCALES)) {
      if (name === 'en') continue
      for (const [key, value] of onboardingBlock(locale)) {
        if (key.startsWith(AUTOSTART_PREFIX) || invariant.has(`onboarding.${key}`)) continue
        expect(value, `${name}: ${key}`).not.toBe(english.get(key))
      }
    }
  })

  it('defines every key the page asks for', () => {
    for (const { template, keys } of TEMPLATE_KEYS) {
      expect(ONBOARDING_SOURCE, template).toContain(`\`${template}\``)
      for (const key of keys) expect(english.has(key), key).toBe(true)
    }
    for (const key of literalKeys(ONBOARDING_SOURCE)) {
      expect(english.has(key), key).toBe(true)
    }
  })

  it('keeps no copy that nothing shows', () => {
    const used = new Set(TEMPLATE_KEYS.flatMap(({ keys }) => keys))
    for (const path of productSources(SRC_ROOT)) {
      for (const key of literalKeys(readFileSync(path, 'utf8'))) used.add(key)
    }
    const unused = [...english.keys()].filter((key) => !used.has(key))
    expect(unused).toEqual([])
  })
})
