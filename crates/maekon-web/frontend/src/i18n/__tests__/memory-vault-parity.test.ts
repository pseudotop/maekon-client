/**
 * ADR-033 (#9465): memory vault mirror i18n 5-locale parity test.
 *
 * Verifies that the (nested-inclusive) key set of `settings.memoryVault` is
 * identical across all 5 locales, and pins the keys `MemoryVaultSettings`
 * actually renders — including the two that carry the ADR's load-bearing
 * disclosures (§3.3 overwrite + sync risk) and every coarse provider label the
 * §3.2 detector can return.
 */

import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const

type JsonRecord = Record<string, unknown>

function vaultBlock(locale: JsonRecord): JsonRecord {
  const settings = locale.settings as JsonRecord | undefined
  const vault = settings?.memoryVault as JsonRecord | undefined
  if (!vault) {
    throw new Error('missing settings.memoryVault block')
  }
  return vault
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

describe('memory vault i18n parity (5 locales)', () => {
  const reference = flatKeys(vaultBlock(en as JsonRecord))

  it('en baseline has the keys the component renders', () => {
    const required = [
      'sectionTitle',
      'sectionDescription',
      'oneWayNotice',
      'enableLabel',
      'enableHint',
      'consentPending',
      'windowOutOfBound',
      'activePathLabel',
      'pathUnresolved',
      'defaultPathNote',
      'pendingPathNote',
      'cloudActiveNotice',
      'pathInputLabel',
      'pathInputPlaceholder',
      'choosePath',
      'useDefault',
      'warningTitle',
      'warningOverwrite',
      'warningSyncDetected',
      'warningSyncUnknown',
      'warningAcknowledge',
      'warningConfirm',
      'warningCancel',
      'pathSaved',
      'pathCleared',
      'exportNow',
      'exportNowHint',
      'exportDone',
      'exportSkipped',
      'exportFailed',
      'conflictsNotice',
      // #9522 §6.4 persisted last-cycle block — the only surface on which a
      // SCHEDULED cycle's marker conflicts are ever reported.
      'lastCycleLabel',
      'lastCycleNever',
      'lastCycleSummary',
      'conflictsTitle',
      'conflictsExplain',
      'conflictsMore',
      // Every coarse label `maekon_core::vault_cloud_sync` can return needs a
      // name, or the warning would interpolate a raw wire label.
      'provider.icloud',
      'provider.cloudStorage',
      'provider.onedrive',
      'provider.dropbox',
      'provider.googleDrive',
    ]
    for (const key of required) {
      expect(reference).toContain(key)
    }
  })

  it('the interpolated keys keep their placeholders in every locale', () => {
    // A locale that drops `{{provider}}` silently turns the §3.3 sync warning
    // into a sentence that names nothing — the exact false-confidence gap the
    // ADR amendment closed.
    const placeholders: Record<string, string> = {
      warningTitle: '{{path}}',
      warningSyncDetected: '{{provider}}',
      cloudActiveNotice: '{{provider}}',
      pendingPathNote: '{{path}}',
      windowOutOfBound: '{{days}}',
      exportSkipped: '{{reason}}',
      conflictsNotice: '{{count}}',
      conflictsTitle: '{{count}}',
      conflictsMore: '{{count}}',
    }
    for (const [name, locale] of Object.entries(LOCALES)) {
      const block = vaultBlock(locale as JsonRecord)
      for (const [key, token] of Object.entries(placeholders)) {
        expect(block[key], `${name}.${key}`).toContain(token)
      }
    }
  })

  it('the last-cycle summary keeps all three of its placeholders in every locale', () => {
    // #9522: one sentence carrying three interpolations. A locale that dropped
    // `{{time}}` would report counts with no "when", which is exactly the
    // per-invocation blindness this block exists to fix.
    for (const [name, locale] of Object.entries(LOCALES)) {
      const value = vaultBlock(locale as JsonRecord).lastCycleSummary as string
      for (const token of ['{{time}}', '{{days}}', '{{expired}}']) {
        expect(value, `${name}.lastCycleSummary`).toContain(token)
      }
    }
  })

  it('the disclosure sentences are real prose in every locale', () => {
    // These four sentences ARE the ADR §1.2/§3.3 user-facing contract; an
    // empty, key-echoing, or placeholder value would ship the feature without
    // its warning. The length floor is deliberately low (12) because CJK
    // locales express the same sentence in far fewer characters than en —
    // an en-calibrated floor would fail zh-CN on correct copy.
    const DISCLOSURES = [
      'oneWayNotice',
      'warningOverwrite',
      'warningSyncUnknown',
      'warningAcknowledge',
      // #9522: the §6.4 "skipped, not overwritten" explanation is the sentence
      // that tells the user their own file is intact and what to do about it.
      'conflictsExplain',
    ]
    for (const [name, locale] of Object.entries(LOCALES)) {
      const block = vaultBlock(locale as JsonRecord)
      for (const key of DISCLOSURES) {
        const value = block[key]
        expect(typeof value, `${name}.${key}`).toBe('string')
        const text = value as string
        expect(text.trim().length, `${name}.${key}`).toBeGreaterThan(12)
        // i18next echoes the key path when a key is missing; a value that IS
        // the key path means the entry was stubbed, not translated.
        expect(text, `${name}.${key}`).not.toContain('settings.memoryVault')
        expect(text, `${name}.${key}`).not.toContain('TODO')
      }
    }
  })

  for (const [name, locale] of Object.entries(LOCALES)) {
    it(`${name} has the same settings.memoryVault key set as en`, () => {
      expect(flatKeys(vaultBlock(locale as JsonRecord))).toEqual(reference)
    })
  }
})
