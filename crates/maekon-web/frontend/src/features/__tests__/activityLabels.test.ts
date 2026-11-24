import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'
import i18n from '../../i18n'
import en from '../../i18n/locales/en.json'
import es from '../../i18n/locales/es.json'
import ja from '../../i18n/locales/ja.json'
import ko from '../../i18n/locales/ko.json'
import zhCN from '../../i18n/locales/zh-CN.json'
import { defaultRegimeLabel, defaultRegimeOptions, KNOWN_WORK_TYPES, workTypeLabel } from '../activityLabels'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const
const t = (key: string) => i18n.t(key)

/** The Rust `WorkType` variants as serde writes them (SCREAMING_SNAKE_CASE). */
function rustWorkTypes(): string[] {
  const source = fs.readFileSync(
    path.resolve(__dirname, '../../../../../maekon-core/src/models/tiered_memory/content.rs'),
    'utf-8',
  )
  const body = source.match(/pub enum WorkType \{([\s\S]*?)\n\}/)?.[1] ?? ''
  return body
    .split('\n')
    .map((line) => line.replace(/\/\/.*$/, '').trim())
    .filter((line) => line.length > 0 && !line.startsWith('#['))
    .map((line) =>
      line
        .replace(/,$/, '')
        .replace(/(?<!^)(?=[A-Z])/g, '_')
        .toUpperCase(),
    )
}

describe('activity labels (#13738)', () => {
  afterEach(async () => {
    await i18n.changeLanguage('en')
  })

  it('knows every work type the Rust core sends', () => {
    const wire = rustWorkTypes()
    // Positive control: the parser found the enum rather than an empty body.
    expect(wire).toContain('ACTIVE_CODING')
    expect([...KNOWN_WORK_TYPES]).toEqual(wire)
  })

  it('has a label for every work type and default regime in every locale', () => {
    const regimeKeys = defaultRegimeOptions((key) => key).map((option) => option.label.replace('regimeOptions.', ''))
    for (const [locale, messages] of Object.entries(LOCALES)) {
      const workTypes = (messages as { workTypes: Record<string, unknown> }).workTypes
      const regimes = (messages as { regimeOptions: Record<string, unknown> }).regimeOptions
      expect(Object.keys(workTypes).sort(), locale).toEqual([...KNOWN_WORK_TYPES].sort())
      expect(Object.keys(regimes).sort(), locale).toEqual([...regimeKeys].sort())
    }
  })

  it('shows Korean names on a Korean screen', async () => {
    await i18n.changeLanguage('ko')

    expect(defaultRegimeOptions(t)).toEqual([
      { id: 'deep-work', label: '집중 업무' },
      { id: 'communication', label: '커뮤니케이션' },
      { id: 'meeting', label: '회의' },
      { id: 'break', label: '휴식' },
      { id: 'admin', label: '사무 업무' },
    ])
    expect(defaultRegimeLabel('meeting', t)).toBe('회의')
    expect(workTypeLabel('ACTIVE_CODING', t)).toBe('코딩')
    expect(workTypeLabel('CODE_REVIEW', t)).toBe('코드 리뷰')
  })

  it('leaves a regime the server named itself to the caller', () => {
    expect(defaultRegimeLabel('regime-3', t)).toBeUndefined()
  })

  it('reads a work type newer than this build as plain words, not a raw id', () => {
    expect(workTypeLabel('SPATIAL_DESIGN', t)).toBe('Spatial Design')
  })
})
