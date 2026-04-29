/**
 * Shared empty/error guard for Reports sections.
 *
 * Lives here (rather than in ReportsLayout) because the layout must always
 * render <Outlet> so the `/reports` → `/reports/activity` index redirect can
 * fire. Each section calls this first and bails out when there is no data
 * to render. See ReportsLayout.tsx for the bug-class context.
 */

import { BarChart3 } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Card, EmptyState } from '../../components/ui'

interface ReportsEmptyStateProps {
  reportError: string | null
}

export function ReportsEmptyState({ reportError }: ReportsEmptyStateProps) {
  const { t } = useTranslation()

  if (reportError) {
    return (
      <Card variant="danger" padding="md">
        <p className="text-semantic-error">{reportError}</p>
      </Card>
    )
  }

  return (
    <EmptyState
      icon={<BarChart3 className="h-8 w-8" />}
      title={t('emptyState.reports.title')}
      description={t('emptyState.reports.description')}
    />
  )
}
