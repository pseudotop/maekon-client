import { describe, expect, it } from 'vitest'
import { formatReportCountTooltipValue } from './ActivityReport'

describe('ActivityReport tooltip formatting', () => {
  it('preserves the metric label and formats the count for chart tooltips', () => {
    expect(formatReportCountTooltipValue(1234, 'Events', 'en-US')).toEqual(['1,234', 'Events'])
  })

  it('falls back to zero for malformed tooltip values', () => {
    expect(formatReportCountTooltipValue(null, 'Events + captures', 'en-US')).toEqual(['0', 'Events + captures'])
  })
})
