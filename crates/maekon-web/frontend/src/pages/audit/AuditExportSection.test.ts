import { describe, expect, it } from 'vitest'
import en from '../../i18n/locales/en.json'
import es from '../../i18n/locales/es.json'
import ja from '../../i18n/locales/ja.json'
import ko from '../../i18n/locales/ko.json'
import zhCn from '../../i18n/locales/zh-CN.json'
import { buildAuditExportFilename } from './AuditExportSection'

// #8081-B: the audit export download must carry a dated, non-colliding filename.
describe('buildAuditExportFilename (#8081-B)', () => {
  it('derives a date-stamped .json filename from an ISO timestamp', () => {
    expect(buildAuditExportFilename('2026-07-11T09:30:00.000Z')).toBe('maekon_audit_export_2026-07-11.json')
  })

  it('uses only the calendar day (drops the time component)', () => {
    expect(buildAuditExportFilename('2026-01-02T23:59:59.999Z')).toBe('maekon_audit_export_2026-01-02.json')
  })
})

describe('audit export privacy copy (#8273)', () => {
  const localeHints = [en, es, ja, ko, zhCn].map((locale) => locale.auditLog.exportHint)

  it.each(localeHints)('does not claim that the redacted file preserves chain metadata: %s', (hint) => {
    expect(hint).not.toMatch(
      /Chain metadata is preserved|체인 메타데이터는 유지|チェーンのメタデータは保持|链元数据将保留|Se conservan los metadatos de la cadena/i,
    )
  })

  it('directs users to the separate integrity verification result', () => {
    expect(en.auditLog.exportHint).toContain('integrity verification above')
    expect(ko.auditLog.exportHint).toContain('무결성 검증 결과')
  })
})
