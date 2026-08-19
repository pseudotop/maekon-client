/**
 * Reports export section (#8081-A, CJ-02-12).
 *
 * The "Export Data" leaf previously rendered a System Metrics CPU+memory trend
 * chart — a duplicate of the /monitoring chart, with zero export affordance.
 * This is now a real, report-scoped export surface that makes scope, format,
 * redaction, and destination explicit before generation, and wires ONLY to
 * export endpoints that already exist on the local dashboard backend:
 *
 *   - Report summary  → serialized client-side from the already-loaded report
 *     (`GET /api/reports`) — an exact JSON snapshot of what the Reports page
 *     shows (daily/hourly/app stats, productivity, totals).
 *   - Metrics / Events / Captures → `GET /api/export/{metrics|events|frames}`
 *     via the existing `exportData` client fn, honoring the JSON/CSV toggle and
 *     the selected date range.
 *
 * No fabricated endpoints. Screenshot pixels are never part of any export (the
 * frame export is metadata + OCR text only).
 */
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { downloadBlob, exportData, exportSessions, type ReportResponse } from '../../api/client'
import type { ExportDataType, ExportFormat, SessionExportKind } from '../../api/contracts'
import { getCaptureReauthStatus, type ReauthStatus, requiresCaptureReauthForExport } from '../../api/reauth'
import { CaptureReauthDialog } from '../../components/CaptureReauthGate'
import { Alert, Button, Card, CardTitle, Input, Select } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { typography } from '../../styles/tokens'
import { ReportsEmptyState } from './ReportsEmptyState'
import type { ReportsContext } from './ReportsLayout'

/** Normalize a date value to the `YYYY-MM-DD` an `<input type="date">` expects. */
function toDateInput(value: string | undefined): string {
  return (value ?? '').slice(0, 10)
}

function rangeSuffix(from?: string, to?: string): string {
  const range = [from, to].filter(Boolean).join('_')
  return range ? `_${range}` : ''
}

/** Build the download filename for a raw data-type export. */
export function buildReportExportFilename(
  dataType: ExportDataType,
  format: ExportFormat,
  from?: string,
  to?: string,
): string {
  return `maekon_report_${dataType}${rangeSuffix(from, to)}.${format}`
}

/** Build the download filename for the report-summary JSON snapshot. */
export function buildReportSummaryFilename(from?: string, to?: string): string {
  return `maekon_report_summary${rangeSuffix(from, to)}.json`
}

/** Serialize the loaded report into a pretty-printed JSON snapshot for download. */
export function serializeReportSummary(report: ReportResponse): string {
  return JSON.stringify(report, null, 2)
}

/** Build the download filename for a work-session interchange export (#9854). */
export function buildSessionExportFilename(kind: SessionExportKind, from?: string, to?: string): string {
  const extension = kind === 'ical' ? 'ics' : 'csv'
  return `maekon_sessions_${kind}${rangeSuffix(from, to)}.${extension}`
}

interface RawExportOption {
  key: ExportDataType
  labelKey: string
  descKey: string
}

const RAW_EXPORTS: RawExportOption[] = [
  { key: 'metrics', labelKey: 'reports.exportMetrics', descKey: 'reports.exportMetricsDesc' },
  { key: 'events', labelKey: 'reports.exportEvents', descKey: 'reports.exportEventsDesc' },
  { key: 'frames', labelKey: 'reports.exportFrames', descKey: 'reports.exportFramesDesc' },
]

interface SessionExportOption {
  key: SessionExportKind
  labelKey: string
  descKey: string
  /** Shown next to the button so the fixed format is visible before clicking. */
  formatLabel: string
}

/**
 * Work-session exports for third-party tools (#9854).
 *
 * Kept out of RAW_EXPORTS because these ignore the JSON/CSV toggle: each
 * endpoint emits the one format its consumer requires. Listing them alongside
 * the toggle-driven exports would suggest the format selector applies.
 */
const SESSION_EXPORTS: SessionExportOption[] = [
  {
    key: 'ical',
    labelKey: 'reports.exportIcal',
    descKey: 'reports.exportIcalDesc',
    formatLabel: '.ics',
  },
  {
    key: 'toggl',
    labelKey: 'reports.exportToggl',
    descKey: 'reports.exportTogglDesc',
    formatLabel: 'CSV',
  },
]

export default function ExportSection() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { report, reportError } = useTypedOutletContext<ReportsContext>('Reports')

  // Explicit scope defaults to the loaded report's window; the user can override.
  const [from, setFrom] = useState(() => toDateInput(report?.from_date))
  const [to, setTo] = useState(() => toDateInput(report?.to_date))
  const [format, setFormat] = useState<ExportFormat>('json')
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [reauthStatus, setReauthStatus] = useState<ReauthStatus | null>(null)

  // Empty/error state is owned by the child (ReportsLayout always renders
  // <Outlet>) — mirror ActivityReport so the /reports index redirect keeps firing.
  if (reportError || !report) {
    return <ReportsEmptyState reportError={reportError} />
  }

  const handleRawExport = async (dataType: ExportDataType, allowReauth = true) => {
    setBusy(dataType)
    setError(null)
    try {
      const blob = await exportData(dataType, format, from || undefined, to || undefined)
      downloadBlob(blob, buildReportExportFilename(dataType, format, from, to))
    } catch (e) {
      if (allowReauth && requiresCaptureReauthForExport(dataType, e)) {
        try {
          const status = await getCaptureReauthStatus()
          if (status.enabled && !status.authenticated) {
            setReauthStatus(status)
          } else {
            // The gate may have opened between the rejected request and the
            // status check. Retry once, without recursively prompting.
            await handleRawExport(dataType, false)
          }
        } catch (statusError) {
          setError(statusError instanceof Error ? statusError.message : t('reauth.errors.failed'))
        }
      } else {
        setError(e instanceof Error ? e.message : t('reports.exportFailed', 'Export failed'))
      }
    } finally {
      setBusy(null)
    }
  }

  const handleSessionExport = async (kind: SessionExportKind) => {
    setBusy(kind)
    setError(null)
    try {
      const blob = await exportSessions(kind, from || undefined, to || undefined)
      downloadBlob(blob, buildSessionExportFilename(kind, from, to))
    } catch (e) {
      // No capture-reauth branch here: these export work sessions (start/end
      // times and titles), never frames, so the capture gate does not apply.
      setError(e instanceof Error ? e.message : t('reports.exportFailed', 'Export failed'))
    } finally {
      setBusy(null)
    }
  }

  const handleSummaryExport = () => {
    setBusy('summary')
    setError(null)
    try {
      const blob = new Blob([serializeReportSummary(report)], { type: 'application/json' })
      downloadBlob(blob, buildReportSummaryFilename(from, to))
    } catch (e) {
      setError(e instanceof Error ? e.message : t('reports.exportFailed', 'Export failed'))
    } finally {
      setBusy(null)
    }
  }

  return (
    <Card id="section-export" padding="md">
      <CardTitle>{t('reports.exportTitle', 'Export report data')}</CardTitle>
      <p className="mt-1 text-content-secondary text-sm">
        {t(
          'reports.exportDescription',
          'Download your activity data for the selected range. Choose the scope and format before generating.',
        )}
      </p>

      {/* Scope: date range + format — explicit before generation. */}
      <div className="mt-4 flex flex-wrap items-end gap-4">
        <label className="flex flex-col gap-1">
          <span className={`${typography.weight.medium} text-content-strong text-sm`}>
            {t('reports.exportFrom', 'From')}
          </span>
          <Input type="date" inputSize="sm" value={from} onChange={(e) => setFrom(e.target.value)} className="w-40" />
        </label>
        <label className="flex flex-col gap-1">
          <span className={`${typography.weight.medium} text-content-strong text-sm`}>
            {t('reports.exportTo', 'To')}
          </span>
          <Input type="date" inputSize="sm" value={to} onChange={(e) => setTo(e.target.value)} className="w-40" />
        </label>
        <label className="flex flex-col gap-1">
          <span className={`${typography.weight.medium} text-content-strong text-sm`}>
            {t('reports.exportFormat', 'Format')}
          </span>
          <Select
            value={format}
            selectSize="sm"
            onChange={(e) => setFormat(e.target.value as ExportFormat)}
            className="w-32"
            data-testid="export-format"
          >
            <option value="json">JSON</option>
            <option value="csv">CSV</option>
          </Select>
        </label>
      </div>

      {/* Report summary snapshot — exactly what the Reports page shows (JSON only). */}
      <div className="mt-6 border-border-subtle border-t pt-4">
        <div className="flex items-center justify-between gap-4">
          <div>
            <span className={`block ${typography.weight.medium} text-content-strong text-sm`}>
              {t('reports.exportSummary', 'Report summary')}
            </span>
            <p className="mt-1 text-content-tertiary text-xs">
              {t(
                'reports.exportSummaryDesc',
                'A JSON snapshot of the charts on this page: daily and hourly activity, app usage, productivity, and totals.',
              )}
            </p>
          </div>
          <Button
            variant="primary"
            size="sm"
            onClick={handleSummaryExport}
            isLoading={busy === 'summary'}
            data-testid="export-summary"
          >
            {t('reports.exportSummaryButton', 'Download JSON')}
          </Button>
        </div>
      </div>

      {/* Raw data-type exports (respect the JSON/CSV toggle + date range). */}
      <div className="mt-4 space-y-3 border-border-subtle border-t pt-4">
        {RAW_EXPORTS.map((option) => (
          <div key={option.key} className="flex items-center justify-between gap-4">
            <div>
              <span className={`block ${typography.weight.medium} text-content-strong text-sm`}>
                {t(option.labelKey)}
              </span>
              <p className="mt-1 text-content-tertiary text-xs">{t(option.descKey)}</p>
            </div>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => handleRawExport(option.key)}
              isLoading={busy === option.key}
              data-testid={`export-${option.key}`}
            >
              {busy === option.key ? t('reports.exporting', 'Exporting…') : t('reports.exportDownload', 'Download')}
            </Button>
          </div>
        ))}
      </div>

      {/* Work-session interchange exports (#9854). Fixed formats — the JSON/CSV
          toggle above does not apply, so the format is labelled per row. */}
      <div className="mt-4 space-y-3 border-border-subtle border-t pt-4">
        <div>
          <span className={`block ${typography.weight.medium} text-content-strong text-sm`}>
            {t('reports.exportSessionsTitle', 'Time tracking')}
          </span>
          <p className="mt-1 text-content-tertiary text-xs">
            {t(
              'reports.exportSessionsDesc',
              'Send your work sessions to a calendar or time-tracking tool. Each format is fixed by the receiving tool, so the format selector above does not apply.',
            )}
          </p>
        </div>
        {SESSION_EXPORTS.map((option) => (
          <div key={option.key} className="flex items-center justify-between gap-4">
            <div>
              <span className={`block ${typography.weight.medium} text-content-strong text-sm`}>
                {t(option.labelKey)}
              </span>
              <p className="mt-1 text-content-tertiary text-xs">{t(option.descKey)}</p>
            </div>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => handleSessionExport(option.key)}
              isLoading={busy === option.key}
              data-testid={`export-${option.key}`}
            >
              {busy === option.key
                ? t('reports.exporting', 'Exporting…')
                : t('reports.exportDownloadAs', 'Download {{format}}', { format: option.formatLabel })}
            </Button>
          </div>
        ))}
      </div>

      {/* Redaction + destination — explicit before generation. */}
      <p className="mt-4 text-content-tertiary text-xs">
        {t(
          'reports.exportRedactionNote',
          'Exports contain activity metadata (app names, window titles, and OCR-extracted text from captures) and system metrics for the selected range. Screenshot images are never included. Files are saved to your browser downloads.',
        )}
      </p>

      {error && (
        <Alert variant="error" title={t('reports.exportFailed', 'Export failed')} className="mt-4">
          {error}
        </Alert>
      )}

      <CaptureReauthDialog
        status={reauthStatus}
        onAuthenticated={async () => {
          setReauthStatus(null)
          await handleRawExport('frames', false)
        }}
        onClose={() => setReauthStatus(null)}
        onGoToSettings={() => {
          setReauthStatus(null)
          navigate('/settings/privacy')
        }}
      />
    </Card>
  )
}
