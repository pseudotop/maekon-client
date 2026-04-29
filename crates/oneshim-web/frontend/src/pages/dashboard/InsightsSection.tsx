/**
 * Insights section — FocusWidget, UpdatePanel, ActivityHeatmap, and System Status cards.
 */

import { useTranslation } from 'react-i18next'
import { ActivityHeatmap } from '../../components/ActivityHeatmap'
import FocusWidget from '../../components/FocusWidget'
import UpdatePanel from '../../components/UpdatePanel'
import { Card, CardTitle } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { DashboardContext } from './DashboardLayout'

export default function InsightsSection() {
  const { t } = useTranslation()
  const { summary, isWidgetVisible } = useTypedOutletContext<DashboardContext>('Dashboard')

  return (
    <>
      {isWidgetVisible('insights.focus-widget') && (
        <div id="section-focus">
          <FocusWidget />
        </div>
      )}

      {isWidgetVisible('insights.update-panel') && (
        <div id="section-updates">
          <UpdatePanel compact />
        </div>
      )}

      {isWidgetVisible('insights.heatmap') && (
        <div id="section-heatmap">
          <ActivityHeatmap days={7} className={colors.surface.elevated} />
        </div>
      )}

      {isWidgetVisible('insights.system-status') && (
        <Card variant="default" padding="lg">
          <CardTitle className="mb-4">{t('dashboard.systemStatus')}</CardTitle>
          <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
            <div className="text-center">
              <div className={cn(typography.stat.hero, colors.primary.text)}>
                {summary?.cpu_avg?.toFixed(1) ?? '0'}%
              </div>
              <div className={cn('text-sm', colors.text.secondary)}>{t('dashboard.avgCpu')}</div>
            </div>
            <div className="text-center">
              <div className={cn(typography.stat.hero, colors.primary.text)}>
                {summary?.memory_avg_percent?.toFixed(1) ?? '0'}%
              </div>
              <div className={cn('text-sm', colors.text.secondary)}>{t('dashboard.avgMemory')}</div>
            </div>
            <div className="text-center">
              <div className={cn(typography.stat.hero, colors.primary.text)}>{summary?.top_apps?.length ?? 0}</div>
              <div className={cn('text-sm', colors.text.secondary)}>{t('dashboard.appsUsed')}</div>
            </div>
            <div className="text-center">
              <div className={cn(typography.stat.hero, colors.primary.text)}>
                {(
                  ((summary?.total_active_secs ?? 0) /
                    Math.max(1, (summary?.total_active_secs ?? 0) + (summary?.total_idle_secs ?? 0))) *
                  100
                ).toFixed(0)}
                %
              </div>
              <div className={cn('text-sm', colors.text.secondary)}>{t('dashboard.activityRatio')}</div>
            </div>
          </div>
        </Card>
      )}
    </>
  )
}
