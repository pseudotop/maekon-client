import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { Outlet } from 'react-router-dom'
import {
  type AutomationStats,
  type AutomationStatus,
  fetchAutomationStats,
  fetchAutomationStatus,
} from '../../api/client'
import { ListSkeleton, Skeleton, StatCardsSkeleton } from '../../components/ui'
import { Badge } from '../../components/ui/Badge'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { colors, typography } from '../../styles/tokens'
import type { BadgeColor } from '../../styles/variants'
import { cn } from '../../utils/cn'

const sourceLabelByValue: Record<string, string> = {
  local: 'automation.sourceLocal',
  remote: 'automation.sourceRemote',
  'local-fallback': 'automation.sourceLocalFallback',
  'cli-subscription': 'automation.sourceCliSubscription',
  platform: 'automation.sourcePlatform',
}

const sourceBadgeColorByValue: Record<string, BadgeColor> = {
  local: 'default',
  remote: 'info',
  'local-fallback': 'warning',
  'cli-subscription': 'success',
  platform: 'primary',
}

type StatTone = 'neutral' | 'success' | 'warning' | 'error'

const statToneClass: Record<StatTone, string> = {
  neutral: 'text-content',
  success: 'text-semantic-success',
  warning: 'text-semantic-warning',
  error: 'text-semantic-error',
}

interface StatMetricProps {
  label: string
  value: string | number
  tone?: StatTone
}

function StatMetric({ label, value, tone = 'neutral' }: StatMetricProps) {
  return (
    <div className="text-center">
      <output aria-label={`${label}: ${value}`} className={cn(typography.stat.large, statToneClass[tone])}>
        {value}
      </output>
      <div className="text-content-secondary text-xs">{label}</div>
    </div>
  )
}

export interface AutomationContext {
  status: AutomationStatus | undefined
  stats: AutomationStats | undefined
}

export default function AutomationLayout() {
  const { t } = useTranslation()

  const { data: status, isLoading: statusLoading } = useQuery({
    queryKey: ['automationStatus'],
    queryFn: fetchAutomationStatus,
    refetchInterval: 30000,
  })

  const { data: stats } = useQuery({
    queryKey: ['automationStats'],
    queryFn: fetchAutomationStats,
    refetchInterval: 30000,
  })

  const ocrFallbackActive = status?.ocr_source === 'local-fallback' || Boolean(status?.ocr_fallback_reason)
  const llmFallbackActive = status?.llm_source === 'local-fallback' || Boolean(status?.llm_fallback_reason)
  const hasFallbackDetails = ocrFallbackActive || llmFallbackActive

  const sourceLabel = (source?: string | null) => t(sourceLabelByValue[source ?? ''] ?? 'automation.sourceUnknown')

  const sourceBadgeColor = (source?: string | null) => sourceBadgeColorByValue[source ?? ''] ?? 'default'
  const totalExecutions = stats?.total_executions ?? 0
  const successful = stats?.successful ?? 0
  const failed = stats?.failed ?? 0
  const denied = stats?.denied ?? 0
  const timeout = stats?.timeout ?? 0
  const successRate = `${((stats?.success_rate ?? 0) * 100).toFixed(1)}%`
  const blockedRate = `${((stats?.blocked_rate ?? 0) * 100).toFixed(1)}%`

  if (statusLoading) {
    return (
      <div className="min-h-full space-y-6 p-6">
        <Skeleton className="h-8 w-48" />
        <StatCardsSkeleton count={3} />
        <Skeleton className="h-10 w-full" />
        <ListSkeleton rows={5} />
      </div>
    )
  }

  // Empty-state UX is owned by PoliciesSection (the defaultChild) so the
  // layout can always render <Outlet>. An earlier revision early-returned
  // EmptyState here when `stats.total_executions === 0 && !status.enabled`,
  // which suppressed the index <Navigate to="policies" replace /> and left
  // `/automation` stuck without redirecting to `/automation/policies`.
  // Same bug class as the AuditLayout empty-state regression.
  const ctx: AutomationContext = { status, stats }

  return (
    <div className="min-h-full space-y-6 p-6">
      <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('automation.title')}</h1>

      {/* Status cards */}
      <div className="grid grid-cols-2 gap-4 md:grid-cols-3 xl:grid-cols-5">
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.status')}</div>
            <div className="mt-1">
              {status?.enabled ? (
                <Badge color="success">{t('automation.enabled')}</Badge>
              ) : (
                <Badge color="default">{t('automation.disabled')}</Badge>
              )}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.sandbox')}</div>
            <div className={`mt-1 ${typography.weight.semibold} text-content text-lg`}>
              {status?.sandbox_profile ?? '-'}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.ocrProvider')}</div>
            <div className={`mt-1 ${typography.weight.semibold} text-content text-lg`}>
              {status?.ocr_provider ?? '-'}
            </div>
            <div className="mt-2 flex items-center gap-2">
              <span className="text-content-secondary text-xs">{t('automation.providerSource')}</span>
              <Badge color={sourceBadgeColor(status?.ocr_source)} size="sm">
                {sourceLabel(status?.ocr_source)}
              </Badge>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.llmProvider')}</div>
            <div className={`mt-1 ${typography.weight.semibold} text-content text-lg`}>
              {status?.llm_provider ?? '-'}
            </div>
            <div className="mt-2 flex items-center gap-2">
              <span className="text-content-secondary text-xs">{t('automation.providerSource')}</span>
              <Badge color={sourceBadgeColor(status?.llm_source)} size="sm">
                {sourceLabel(status?.llm_source)}
              </Badge>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.pendingAudit')}</div>
            <div className={`mt-1 ${typography.weight.semibold} text-content text-lg`}>
              {status?.pending_audit_entries ?? 0}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Fallback details */}
      {hasFallbackDetails && (
        <Card>
          <CardHeader>
            <CardTitle>{t('automation.fallbackDetails')}</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="space-y-3 text-sm">
              {ocrFallbackActive && (
                <div>
                  <div className="flex items-center gap-2">
                    <span className={`${typography.weight.medium} text-content`}>{t('automation.ocrProvider')}</span>
                    <Badge color={sourceBadgeColor(status?.ocr_source)} size="sm">
                      {sourceLabel(status?.ocr_source)}
                    </Badge>
                  </div>
                  <div className="mt-1 text-content-secondary">
                    {t('automation.fallbackReason')}:{' '}
                    {status?.ocr_fallback_reason ?? t('automation.fallbackReasonUnavailable')}
                  </div>
                </div>
              )}
              {llmFallbackActive && (
                <div>
                  <div className="flex items-center gap-2">
                    <span className={`${typography.weight.medium} text-content`}>{t('automation.llmProvider')}</span>
                    <Badge color={sourceBadgeColor(status?.llm_source)} size="sm">
                      {sourceLabel(status?.llm_source)}
                    </Badge>
                  </div>
                  <div className="mt-1 text-content-secondary">
                    {t('automation.fallbackReason')}:{' '}
                    {status?.llm_fallback_reason ?? t('automation.fallbackReasonUnavailable')}
                  </div>
                </div>
              )}
            </div>
          </CardContent>
        </Card>
      )}

      {/* Stats card */}
      <Card>
        <CardHeader>
          <CardTitle>{t('automation.statsTitle')}</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-2 gap-4 md:grid-cols-5">
            <StatMetric label={t('automation.totalExecutions')} value={totalExecutions} />
            <StatMetric
              label={t('automation.successful')}
              value={successful}
              tone={successful > 0 ? 'success' : 'neutral'}
            />
            <StatMetric label={t('automation.failed')} value={failed} tone={failed > 0 ? 'error' : 'neutral'} />
            <StatMetric label={t('automation.denied')} value={denied} tone={denied > 0 ? 'warning' : 'neutral'} />
            <StatMetric label={t('automation.timeout')} value={timeout} tone={timeout > 0 ? 'warning' : 'neutral'} />
            <StatMetric
              label={t('automation.successRate')}
              value={successRate}
              tone={totalExecutions > 0 && (stats?.success_rate ?? 0) > 0 ? 'success' : 'neutral'}
            />
            <StatMetric
              label={t('automation.blockedRate')}
              value={blockedRate}
              tone={(stats?.blocked_rate ?? 0) > 0 ? 'warning' : 'neutral'}
            />
            <StatMetric label={t('automation.avgElapsed')} value={`${(stats?.avg_elapsed_ms ?? 0).toFixed(0)}ms`} />
            <StatMetric label={t('automation.p95Elapsed')} value={`${(stats?.p95_elapsed_ms ?? 0).toFixed(0)}ms`} />
            <StatMetric label={t('automation.timingSamples')} value={stats?.timing_samples ?? 0} />
          </div>
        </CardContent>
      </Card>

      <Outlet context={ctx} />
    </div>
  )
}
