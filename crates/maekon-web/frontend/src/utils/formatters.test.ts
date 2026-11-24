import { afterEach, describe, expect, it } from 'vitest'
import i18n from '../i18n'
import {
  formatBytes,
  formatClock,
  formatDate,
  formatDateTime,
  formatDuration,
  formatGigabytes,
  formatHours,
  formatMinutes,
  formatNumber,
  hoursFromSeconds,
  largestElapsedUnit,
  minutesFromSeconds,
} from './formatters'

describe('locale-aware formatters', () => {
  it('formats dates and numbers with the requested locale', () => {
    expect(formatDate('2026-04-27T09:05:06.000Z', 'en-US')).toMatch(/Apr/)
    expect(formatDateTime('2026-04-27T09:05:06.000Z', 'en-US')).toMatch(/Apr/)
    expect(formatNumber(1234567, 'en-US')).toBe('1,234,567')
  })

  it('formats memory values for user-facing metric surfaces', () => {
    expect(formatGigabytes(12.690163866616786)).toBe('12.7GB')
    expect(formatBytes(8_589_934_592)).toBe('8.0GB')
  })
})

// #13740: one rule for every screen — seconds under a minute, whole minutes
// under an hour, then hours and minutes. Seconds are dropped from a minute on.
describe('formatDuration', () => {
  afterEach(async () => {
    await i18n.changeLanguage('en')
  })

  const RULE: ReadonlyArray<[seconds: number, en: string, ko: string]> = [
    [0, '0s', '0초'],
    [45, '45s', '45초'],
    [59, '59s', '59초'],
    [60, '1m', '1분'],
    [425, '7m', '7분'],
    [3599, '59m', '59분'],
    [3600, '1h', '1시간'],
    [21_600, '6h', '6시간'],
    [22_020, '6h 7m', '6시간 7분'],
    [22_079, '6h 7m', '6시간 7분'],
    [90_000, '25h', '25시간'],
  ]

  it.each(RULE)('formats %s seconds as "%s" in English', async (seconds, en) => {
    await i18n.changeLanguage('en')
    expect(formatDuration(seconds)).toBe(en)
  })

  it.each(RULE)('formats %s seconds with Korean units (%s → %s)', async (seconds, _en, ko) => {
    await i18n.changeLanguage('ko')
    expect(formatDuration(seconds)).toBe(ko)
  })

  it('follows the active language', async () => {
    // zh-CN spelling is pinned at the resource level in time-format-parity.test.ts:
    // with `nonExplicitSupportedLngs` the runtime currently resolves 'zh-CN' to English.
    const expected: Record<string, string> = {
      en: '6h 7m',
      ko: '6시간 7분',
      ja: '6時間7分',
      es: '6 h 7 min',
    }
    for (const [language, text] of Object.entries(expected)) {
      await i18n.changeLanguage(language)
      expect(formatDuration(22_020), language).toBe(text)
    }
  })

  it('gives the same text whether or not a caller still passes the old short flag', async () => {
    await i18n.changeLanguage('en')
    for (const [seconds] of RULE) {
      expect(formatDuration(seconds, true)).toBe(formatDuration(seconds))
      expect(formatDuration(seconds, false)).toBe(formatDuration(seconds))
    }
  })

  it('treats fractional, negative and non-finite input as whole non-negative seconds', async () => {
    await i18n.changeLanguage('en')
    expect(formatDuration(59.9)).toBe('59s')
    expect(formatDuration(-5)).toBe('0s')
    expect(formatDuration(Number.NaN)).toBe('0s')
    expect(formatDuration(Number.POSITIVE_INFINITY)).toBe('0s')
  })
})

// The API reports some durations in minutes or fractional hours; they go
// through the same rule so the same length reads the same on every screen.
describe('formatMinutes and formatHours', () => {
  afterEach(async () => {
    await i18n.changeLanguage('en')
  })

  it('formats minute counts with the shared rule', async () => {
    await i18n.changeLanguage('en')
    expect(formatMinutes(30)).toBe('30m')
    expect(formatMinutes(60)).toBe('1h')
    expect(formatMinutes(135)).toBe(formatDuration(135 * 60))
    expect(formatMinutes(135)).toBe('2h 15m')
  })

  it('formats fractional hours with the shared rule', async () => {
    await i18n.changeLanguage('en')
    expect(formatHours(2.25)).toBe('2h 15m')
    expect(formatHours(0.5)).toBe('30m')
    expect(formatHours(1)).toBe('1h')
    // 2.3333333 h is 8399.99988 s; rounding first keeps it at 140 minutes.
    expect(formatHours(2.3333333)).toBe('2h 20m')
    expect(formatHours(0)).toBe(formatDuration(0))
  })

  it('uses Korean units on a Korean screen', async () => {
    await i18n.changeLanguage('ko')
    expect(formatMinutes(135)).toBe('2시간 15분')
    expect(formatHours(0.5)).toBe('30분')
  })

  it('never renders a negative or broken length', async () => {
    await i18n.changeLanguage('en')
    expect(formatMinutes(-5)).toBe('0s')
    expect(formatHours(Number.NaN)).toBe('0s')
  })
})

describe('formatClock', () => {
  it('renders a countdown as MM:SS and keeps counting minutes past an hour', () => {
    expect(formatClock(0)).toBe('00:00')
    expect(formatClock(65)).toBe('01:05')
    expect(formatClock(1500)).toBe('25:00')
    expect(formatClock(7500)).toBe('125:00')
  })

  it('never shows a negative or broken clock', () => {
    expect(formatClock(-1)).toBe('00:00')
    expect(formatClock(Number.NaN)).toBe('00:00')
    expect(formatClock(61.7)).toBe('01:01')
  })
})

describe('minutesFromSeconds', () => {
  it('rounds to the nearest whole minute for minute-based controls', () => {
    expect(minutesFromSeconds(0)).toBe(0)
    expect(minutesFromSeconds(29)).toBe(0)
    expect(minutesFromSeconds(30)).toBe(1)
    expect(minutesFromSeconds(420)).toBe(7)
    expect(minutesFromSeconds(3600)).toBe(60)
    expect(minutesFromSeconds(-60)).toBe(0)
  })
})

describe('hoursFromSeconds', () => {
  it('converts a seconds total to fractional hours for comparison with hour-based API values', () => {
    expect(hoursFromSeconds(5400)).toBe(1.5)
    expect(hoursFromSeconds(3600)).toBe(1)
    expect(hoursFromSeconds(0)).toBe(0)
    expect(hoursFromSeconds(-3600)).toBe(0)
    expect(hoursFromSeconds(Number.NaN)).toBe(0)
    // The result feeds formatHours, which renders it with the shared rule.
    expect(formatHours(hoursFromSeconds(5400))).toBe('1h 30m')
  })
})

describe('largestElapsedUnit', () => {
  const MINUTE = 60_000
  const HOUR = 60 * MINUTE
  const DAY = 24 * HOUR

  it('picks the largest whole unit and never reports zero minutes', () => {
    expect(largestElapsedUnit(0)).toEqual({ unit: 'minutes', count: 1 })
    expect(largestElapsedUnit(59 * 1000)).toEqual({ unit: 'minutes', count: 1 })
    expect(largestElapsedUnit(5 * MINUTE)).toEqual({ unit: 'minutes', count: 5 })
    expect(largestElapsedUnit(HOUR - 1)).toEqual({ unit: 'minutes', count: 59 })
    expect(largestElapsedUnit(HOUR)).toEqual({ unit: 'hours', count: 1 })
    expect(largestElapsedUnit(DAY - 1)).toEqual({ unit: 'hours', count: 23 })
    expect(largestElapsedUnit(3 * DAY + 5 * HOUR)).toEqual({ unit: 'days', count: 3 })
  })

  it('clamps negative and non-finite input to the smallest bucket', () => {
    expect(largestElapsedUnit(-HOUR)).toEqual({ unit: 'minutes', count: 1 })
    expect(largestElapsedUnit(Number.NaN)).toEqual({ unit: 'minutes', count: 1 })
  })
})
