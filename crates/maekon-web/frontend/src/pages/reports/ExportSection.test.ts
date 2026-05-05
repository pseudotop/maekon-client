import { describe, expect, it } from 'vitest'
import { EXPORT_METRIC_COLORS, formatExportMetricTooltipValue } from './ExportSection'

describe('ExportSection metric chart formatting', () => {
  it('keeps metric labels visible and formats values as percentages', () => {
    expect(formatExportMetricTooltipValue(12.956, 'CPU')).toEqual(['13.0%', 'CPU'])
    expect(formatExportMetricTooltipValue(79.824, 'Memory')).toEqual(['79.8%', 'Memory'])
  })

  it('uses a memory line color that matches the memory legend swatch', () => {
    expect(EXPORT_METRIC_COLORS.memoryStroke).toBe('#14b8a6')
    expect(EXPORT_METRIC_COLORS.memoryLegendStyle.backgroundColor).toBe(EXPORT_METRIC_COLORS.memoryStroke)
  })
})
