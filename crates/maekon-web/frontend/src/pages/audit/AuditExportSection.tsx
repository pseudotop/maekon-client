/**
 * #8081-B (CJ-04-15): audit-trail export affordance. The audit summary was
 * visible but had no export path — the backend `GET /api/audit/export` handler
 * (DoS-capped at 1000 entries server-side) had zero frontend callers. This
 * "Export audit entries" button fetches that canonical export and streams a
 * JSON snapshot to a browser download.
 *
 * The exported entries carry only display evidence metadata; session IDs and
 * free-form details are omitted. Chain integrity is verified separately by the
 * verification report above. Rendered unconditionally by
 * SummarySection — including on the empty-state path — so the capability stays
 * reachable even with zero logged executions (an empty export is a valid `[]`).
 */
import { Download } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { downloadBlob, fetchAuditExport } from '../../api/client'
import { Alert, Button, Spinner } from '../../components/ui'
import { colors, iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

/** Build the audit-export download filename from an ISO timestamp (YYYY-MM-DD). */
export function buildAuditExportFilename(nowIso: string): string {
  const day = nowIso.slice(0, 10)
  return `maekon_audit_export_${day}.json`
}

/** Server-side DoS cap for the audit export endpoint (mirrors the backend clamp). */
const AUDIT_EXPORT_LIMIT = 1000

export default function AuditExportSection() {
  const { t } = useTranslation()
  const [exporting, setExporting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const handleExport = async () => {
    setExporting(true)
    setError(null)
    try {
      const entries = await fetchAuditExport({ limit: AUDIT_EXPORT_LIMIT })
      const blob = new Blob([JSON.stringify(entries, null, 2)], { type: 'application/json' })
      downloadBlob(blob, buildAuditExportFilename(new Date().toISOString()))
    } catch (e) {
      setError(e instanceof Error ? e.message : t('auditLog.exportFailed', 'Export failed'))
    } finally {
      setExporting(false)
    }
  }

  return (
    <div id="section-audit-export" className="space-y-3 rounded-lg border border-DEFAULT p-4">
      <div className="flex items-center justify-between gap-4">
        <div>
          <h2 className={cn(typography.label, colors.text.primary)}>
            {t('auditLog.exportTitle', 'Export audit trail')}
          </h2>
          <p className={cn(typography.caption, colors.text.secondary)}>
            {t(
              'auditLog.exportHint',
              'Download the most recent audit entries (up to 1000) as a JSON file. Only evidence metadata is included; use the integrity verification above to validate the stored chain.',
            )}
          </p>
        </div>
        <Button variant="secondary" size="sm" onClick={handleExport} disabled={exporting} data-testid="audit-export">
          {exporting ? (
            <Spinner size="sm" />
          ) : (
            <span className="flex items-center gap-2">
              <Download className={iconSize.base} />
              {t('auditLog.exportButton', 'Export audit entries')}
            </span>
          )}
        </Button>
      </div>

      {error && (
        <Alert variant="error" title={t('auditLog.exportFailed', 'Export failed')}>
          {error}
        </Alert>
      )}
    </div>
  )
}
