/**
 * 마이크 동의 i18n 5-로케일 패리티 테스트 (#4568 Phase B, design §4/§7).
 *
 * ConsentToggleSection 행위 테스트(A.2 suite)는 fallbackLng='en' 이라 en 으로만 해석되어
 * 비-en 로케일의 키 누락을 잡지 못한다. 이 테스트는 5개 JSON 을 *직접* 로드해
 * `privacy.consent.microphone.*` 의 (중첩 포함) 키-셋이 5 로케일 전부 동일한지 단언한다.
 * 어느 한 로케일에서 키를 빠뜨리면(또는 오타) 이 테스트가 실패한다.
 */

import { describe, expect, it } from 'vitest'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'

const LOCALES = { en, ko, ja, 'zh-CN': zhCN, es } as const

type JsonRecord = Record<string, unknown>

/** privacy.consent.microphone 블록을 꺼낸다. 없으면 테스트가 명시적으로 실패하도록 한다. */
function micBlock(locale: JsonRecord): JsonRecord {
  const privacy = locale.privacy as JsonRecord | undefined
  const consent = privacy?.consent as JsonRecord | undefined
  const microphone = consent?.microphone as JsonRecord | undefined
  if (!microphone) {
    throw new Error('privacy.consent.microphone 블록이 없습니다')
  }
  return microphone
}

/** 중첩 객체를 점-구분 경로의 정렬된 키 목록으로 평탄화한다(리프 키만). */
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

describe('privacy.consent.microphone i18n parity (5 locales)', () => {
  // en 을 기준 키-셋으로 삼는다.
  const reference = flatKeys(micBlock(en as JsonRecord))

  it('en defines the full expected microphone key-set', () => {
    // 설계가 요구하는 정확한 리프 키 목록(회귀 방지용 명시적 단언).
    expect(reference).toEqual([
      'collected.audioCapture',
      'collected.cloudEgress',
      'collected.transcript',
      'collectedTitle',
      'description',
      'disclosure',
      'label',
      'osNote',
      'upgradeNotice.body',
      'upgradeNotice.cta',
      'upgradeNotice.dismiss',
      'upgradeNotice.title',
    ])
  })

  for (const [name, locale] of Object.entries(LOCALES)) {
    it(`${name} has an identical microphone key-set to en`, () => {
      expect(flatKeys(micBlock(locale as JsonRecord))).toEqual(reference)
    })

    it(`${name} microphone values are all non-empty strings`, () => {
      const block = micBlock(locale as JsonRecord)
      for (const path of reference) {
        const value = path.split('.').reduce<unknown>((acc, seg) => (acc as JsonRecord)[seg], block)
        expect(typeof value, `${name}.${path}`).toBe('string')
        expect((value as string).trim().length, `${name}.${path}`).toBeGreaterThan(0)
      }
    })
  }
})
