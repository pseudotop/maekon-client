/**
 * Reports focus/productivity section — summary card and productivity metrics.
 */

import { useTranslation } from 'react-i18next'
import { Card, CardTitle } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { typography } from '../../styles/tokens'
import { formatDuration } from '../../utils/formatters'
import { ReportsEmptyState } from './ReportsEmptyState'
import type { ReportsContext } from './ReportsLayout'

function getScoreColor(score: number): string {
  if (score >= 80) return 'text-semantic-success'
  if (score >= 60) return 'text-brand-text'
  if (score >= 40) return 'text-semantic-warning'
  return 'text-semantic-error'
}

function TrendIndicator({ trend }: { trend: number }) {
  if (trend > 5) {
    return <span className="text-semantic-success">↑ {trend.toFixed(1)}%</span>
  }
  if (trend < -5) {
    return <span className="text-semantic-error">↓ {Math.abs(trend).toFixed(1)}%</span>
  }
  return <span className="text-content-tertiary">→ {trend.toFixed(1)}%</span>
}

export default function FocusReport() {
  const { t } = useTranslation()
  const { report, reportError } = useTypedOutletContext<ReportsContext>('Reports')

  if (reportError || !report) {
    return <ReportsEmptyState reportError={reportError} />
  }

  return (
    <>
      {/* Report summary card */}
      <Card padding="md">
        <div className="flex flex-col justify-between gap-4 md:flex-row md:items-center">
          <div>
            <h2 className={`${typography.weight.bold} text-content text-xl`}>{report.title}</h2>
            <p className="text-content-secondary text-sm">
              {report.from_date} ~ {report.to_date} ({report.days} {t('reports.days')})
            </p>
          </div>
          <div className="flex items-center gap-4">
            {/* Productivity score */}
            <div className="text-center">
              <p className={`${typography.weight.bold} text-4xl ${getScoreColor(report.productivity.score)}`}>
                {report.productivity.score.toFixed(0)}
              </p>
              <p className="text-content-secondary text-xs">{t('reports.productivityScore')}</p>
            </div>
            {/* Trend */}
            <div className="text-center">
              <p className={`${typography.weight.semibold} text-xl`}>
                <TrendIndicator trend={report.productivity.trend} />
              </p>
              <p className="text-content-secondary text-xs">{t('reports.trend')}</p>
            </div>
          </div>
        </div>
      </Card>

      {/* Stat cards */}
      <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
        <Card padding="md">
          <p className="text-content-secondary text-sm">{t('reports.activeTime')}</p>
          <p className={`${typography.weight.bold} text-2xl text-content`}>
            {formatDuration(report.total_active_secs)}
          </p>
        </Card>
        <Card padding="md">
          <p className="text-content-secondary text-sm">{t('reports.idleTime')}</p>
          <p className={`${typography.weight.bold} text-2xl text-content`}>{formatDuration(report.total_idle_secs)}</p>
        </Card>
        <Card padding="md">
          <p className="text-content-secondary text-sm">{t('reports.captures')}</p>
          <p className={`${typography.weight.bold} text-2xl text-content`}>{report.total_captures.toLocaleString()}</p>
        </Card>
        <Card padding="md">
          <p className="text-content-secondary text-sm">{t('reports.events')}</p>
          <p className={`${typography.weight.bold} text-2xl text-content`}>{report.total_events.toLocaleString()}</p>
        </Card>
      </div>

      {/* Productivity metrics */}
      <Card id="section-focus" padding="md">
        <CardTitle>{t('reports.productivityMetrics')}</CardTitle>
        <div className="mt-4 grid grid-cols-2 gap-4 md:grid-cols-4">
          <div>
            <p className="text-content-secondary text-sm">{t('reports.activeRatio')}</p>
            <p className={`${typography.weight.semibold} text-content text-xl`}>
              {(report.productivity.active_ratio ?? 0).toFixed(1)}%
            </p>
          </div>
          <div>
            <p className="text-content-secondary text-sm">{t('reports.peakHour')}</p>
            <p className={`${typography.weight.semibold} text-content text-xl`}>{report.productivity.peak_hour}:00</p>
          </div>
          <div>
            <p className="text-content-secondary text-sm">{t('reports.topApp')}</p>
            <p className={`truncate ${typography.weight.semibold} text-content text-xl`}>
              {report.productivity.top_app || '-'}
            </p>
          </div>
          <div>
            <p className="text-content-secondary text-sm">{t('reports.avgCpu')}</p>
            <p className={`${typography.weight.semibold} text-content text-xl`}>{(report.avg_cpu ?? 0).toFixed(1)}%</p>
          </div>
        </div>
      </Card>
    </>
  )
}
