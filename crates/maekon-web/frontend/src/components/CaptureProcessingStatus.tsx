import { useTranslation } from 'react-i18next'
import type { DailySummary } from '../api/contracts'
import { colors, typography } from '../styles/tokens'
import { formatDuration } from '../utils/formatters'
import { Card, CardContent, CardTitle } from './ui'

export function hasCapturedActivity(summary: DailySummary | null | undefined): summary is DailySummary {
  return Boolean(summary && (summary.total_active_secs > 0 || summary.frames_captured > 0 || summary.events_logged > 0))
}

export default function CaptureProcessingStatus({ summary }: { summary: DailySummary }) {
  const { t } = useTranslation()

  return (
    <Card variant="default" padding="md">
      <CardTitle>{t('dashboard.capturePending.title')}</CardTitle>
      <CardContent>
        <p className={`mb-3 text-sm ${colors.text.secondary}`}>{t('dashboard.capturePending.description')}</p>
        <dl className="grid grid-cols-3 gap-3 text-sm">
          <div>
            <dt className={colors.text.secondary}>{t('dashboard.activeTime')}</dt>
            <dd className={typography.weight.semibold}>{formatDuration(summary.total_active_secs)}</dd>
          </div>
          <div>
            <dt className={colors.text.secondary}>{t('dashboard.captures')}</dt>
            <dd className={typography.weight.semibold}>{summary.frames_captured.toLocaleString()}</dd>
          </div>
          <div>
            <dt className={colors.text.secondary}>{t('dashboard.events')}</dt>
            <dd className={typography.weight.semibold}>{summary.events_logged.toLocaleString()}</dd>
          </div>
        </dl>
      </CardContent>
    </Card>
  )
}
