import { act, renderHook } from '@testing-library/react'
import { createInstance } from 'i18next'
import { createElement, type PropsWithChildren } from 'react'
import { I18nextProvider, initReactI18next } from 'react-i18next'
import { describe, expect, it } from 'vitest'
import {
  describeErrorForDisplay,
  GENERIC_ERROR_KEY,
  LocalizedError,
  supportDetailOf,
  useErrorDisplay,
} from '../errorDisplay'
import en from '../locales/en.json'
import es from '../locales/es.json'
import ja from '../locales/ja.json'
import ko from '../locales/ko.json'
import zhCN from '../locales/zh-CN.json'
import wireEn from '../wire-errors.en.json'
import wireEs from '../wire-errors.es.json'
import wireJa from '../wire-errors.ja.json'
import wireKo from '../wire-errors.ko.json'
import wireZhCN from '../wire-errors.zh-CN.json'

const WIRE_LOCALES = { en: wireEn, ko: wireKo, ja: wireJa, 'zh-CN': wireZhCN, es: wireEs } as const

/** Stand-in for `t`: keys come back as-is so the resolution step is visible. */
const keyOf = (key: string) => `t(${key})`

describe('describeErrorForDisplay (#13737)', () => {
  it('uses the UI key of an out-of-registry Tauri code and needs no detail', () => {
    expect(
      describeErrorForDisplay({ code: 'storage.unavailable', message: 'sqlite pool closed' }, keyOf, 'ko'),
    ).toEqual({ text: 't(errors.ipc.storageUnavailable)', detail: null })
  })

  it('honours a surface-specific lookup before the shared table', () => {
    const display = describeErrorForDisplay(
      { code: 'auth.failed', message: 'HTTP 401' },
      keyOf,
      'ko',
      undefined,
      () => 'settings.account.login.errors.invalidCredentials',
    )

    expect(display).toEqual({ text: 't(settings.account.login.errors.invalidCredentials)', detail: null })
  })

  it.each([
    ['ko', 'policy.denied', wireKo['policy.denied']],
    ['en', 'policy.denied', wireEn['policy.denied']],
    ['ko-KR', 'service.unavailable', wireKo['service.unavailable']],
    ['ja', 'policy.denied', wireJa['policy.denied']],
    ['ja-JP', 'policy.denied', wireJa['policy.denied']],
    ['zh-CN', 'policy.denied', wireZhCN['policy.denied']],
    ['es', 'policy.denied', wireEs['policy.denied']],
    ['es-MX', 'policy.denied', wireEs['policy.denied']],
    ['es-419', 'policy.denied', wireEs['policy.denied']],
  ])('shows the %s catalog sentence for %s without its English message slot', (language, code, template) => {
    const display = describeErrorForDisplay({ code, message: 'egress budget exhausted' }, keyOf, language)

    expect(template).toContain('{message}')
    expect(display.text).toBe(template.replace(/\s*[:：]?\s*\{message\}\s*$/u, ''))
    expect(display.text).not.toContain('{message}')
    expect(display.text).not.toContain('egress budget exhausted')
    expect(display.detail).toBe(`${code}: egress budget exhausted`)
  })

  it('shows a slotless catalog sentence whole', () => {
    expect(
      describeErrorForDisplay({ code: 'autostart.enable_failed', message: 'launchd refused' }, keyOf, 'ko'),
    ).toEqual({ text: wireKo['autostart.enable_failed'], detail: null })
  })

  it.each(Object.entries(WIRE_LOCALES))('shows a slotless %s catalog sentence whole', (language, catalog) => {
    expect(describeErrorForDisplay({ code: 'consent.expired', message: 'backend detail' }, keyOf, language)).toEqual({
      text: catalog['consent.expired'],
      detail: null,
    })
  })

  it('keeps an unknown code out of the text and in the detail', () => {
    expect(
      describeErrorForDisplay(
        { code: 'focus.engine_stalled', message: 'tick lag 9s' },
        keyOf,
        'ko',
        '저장하지 못했습니다.',
      ),
    ).toEqual({ text: '저장하지 못했습니다.', detail: 'focus.engine_stalled: tick lag 9s' })
  })

  it('falls back to the generic sentence when the caller gives none', () => {
    expect(describeErrorForDisplay(new Error('Tag query failed'), keyOf, 'ko')).toEqual({
      text: `t(${GENERIC_ERROR_KEY})`,
      detail: 'Tag query failed',
    })
  })

  it.each([
    ['a plain Error', new Error('Request failed with status 500'), 'Request failed with status 500'],
    ['a TypeError', new TypeError('Failed to fetch'), 'TypeError: Failed to fetch'],
    ['a legacy string', 'Authentication error [auth.failed]: denied', 'Authentication error [auth.failed]: denied'],
  ])('shows the fallback for %s and keeps its text as detail', (_label, error, detail) => {
    expect(describeErrorForDisplay(error, keyOf, 'ko', '불러오지 못했습니다.')).toEqual({
      text: '불러오지 못했습니다.',
      detail,
    })
  })

  it('shows a LocalizedError as written', () => {
    expect(describeErrorForDisplay(new LocalizedError('후보를 선택하세요.'), keyOf, 'ko', 'unused')).toEqual({
      text: '후보를 선택하세요.',
      detail: null,
    })
  })
})

describe('useErrorDisplay supplied-language boundary (#13756)', () => {
  it('follows actual i18next language changes while retaining safe text and support detail', async () => {
    // This provider supplies the language; application detection and persistence belong to #13749.
    const i18n = createInstance()
    await i18n.use(initReactI18next).init({
      lng: 'en',
      fallbackLng: 'en',
      resources: {
        en: { translation: en },
        ko: { translation: ko },
        ja: { translation: ja },
        'zh-CN': { translation: zhCN },
        es: { translation: es },
      },
      interpolation: { escapeValue: false },
      react: { useSuspense: false },
    })
    const wrapper = ({ children }: PropsWithChildren) => createElement(I18nextProvider, { i18n }, children)
    const { result } = renderHook(() => useErrorDisplay(), { wrapper })
    const error = { code: 'policy.denied', message: 'egress budget exhausted' }

    for (const [language, catalog] of Object.entries(WIRE_LOCALES)) {
      await act(async () => {
        await i18n.changeLanguage(language)
      })
      const display = result.current(error)
      expect(display.text).toBe(catalog['policy.denied'].replace(/\s*[:：]?\s*\{message\}\s*$/u, ''))
      expect(display.text).not.toContain(error.message)
      expect(display.detail).toBe('policy.denied: egress budget exhausted')
      expect(result.current({ code: 'future.unknown', message: error.message })).toEqual({
        text: i18n.t(GENERIC_ERROR_KEY),
        detail: 'future.unknown: egress budget exhausted',
      })
    }
  })
})

describe('supportDetailOf (#13737)', () => {
  it.each([
    [
      'an IpcError',
      { code: 'policy.denied', message: ' egress budget exhausted ' },
      'policy.denied: egress budget exhausted',
    ],
    ['an IpcError without a message', { code: 'policy.denied', message: '' }, 'policy.denied'],
    ['a plain Error', new Error('boom'), 'boom'],
    ['a named Error', new RangeError('out of range'), 'RangeError: out of range'],
    ['an Error without a message', new TypeError(''), 'TypeError'],
    ['a string', '  raw text  ', 'raw text'],
    ['an object with a message', { message: 'stream closed' }, 'stream closed'],
    ['an empty string', '   ', null],
    ['an object without a message', { status: 500 }, null],
    ['null', null, null],
  ])('reads %s', (_label, error, expected) => {
    expect(supportDetailOf(error)).toBe(expected)
  })
})
