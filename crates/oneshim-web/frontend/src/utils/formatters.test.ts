import { describe, expect, it } from 'vitest'
import { formatBytes, formatDate, formatDateTime, formatGigabytes, formatNumber } from './formatters'

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
