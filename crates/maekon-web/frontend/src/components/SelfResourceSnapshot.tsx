/**
 * #8082 / #8058: production UI consumer for the `get_resource_usage_snapshot`
 * Tauri IPC command, which previously had no caller. It samples the desktop
 * agent's OWN process RSS + CPU and compares them against the provisional
 * resource budget SSOT — the read side of the "<2% CPU, you won't notice it
 * running" claim made measurable.
 *
 * This is a LOCAL-ONLY self-process diagnostic: only the desktop webview can
 * sample its own process, so — following the sibling `get_runtime_log_snapshot`
 * consumer (GeneralTab) — it invokes the command in Tauri context only and
 * degrades to a short "desktop app" note in a plain browser. No REST handler is
 * added (a remote browser cannot sample this process), so no OpenAPI change.
 *
 * `measured: false` (unsupported OS / restricted process visibility) renders
 * "n/a" rather than a misleading 0, and never signals a false budget breach.
 */
import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { colors, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { formatBytes, formatPercent } from '../utils/formatters'
import { IS_TAURI } from '../utils/platform'
import { Alert, Badge, Button, Card, CardTitle, Spinner } from './ui'

/** Local diagnostics snapshot of the agent's own process usage vs. its budget. */
export interface ResourceUsageSnapshot {
  generated_at: string
  rss_bytes: number
  cpu_percent: number
  rss_budget_bytes: number
  cpu_budget_percent: number
  rss_within_budget: boolean
  cpu_within_budget: boolean
  measured: boolean
}

async function invokeResourceSnapshot(): Promise<ResourceUsageSnapshot> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<ResourceUsageSnapshot>('get_resource_usage_snapshot')
}

function MetricRow({
  label,
  measured,
  value,
  budget,
  withinBudget,
}: {
  label: string
  measured: boolean
  value: string
  budget: string
  withinBudget: boolean
}) {
  const { t } = useTranslation()
  return (
    <div className="flex items-center justify-between gap-4 py-1">
      <span className="text-content-secondary text-sm">{label}</span>
      <div className="flex items-center gap-3">
        <span className={cn(typography.weight.medium, colors.text.primary, 'text-sm')}>
          {measured ? value : t('metrics.notMeasured', 'n/a')}
        </span>
        <span className="text-content-tertiary text-xs">/ {budget}</span>
        {measured && (
          <Badge color={withinBudget ? 'default' : 'warning'} size="sm">
            {withinBudget ? t('metrics.withinBudget', 'Within budget') : t('metrics.overBudget', 'Over budget')}
          </Badge>
        )}
      </div>
    </div>
  )
}

export default function SelfResourceSnapshot() {
  const { t } = useTranslation()

  const {
    data: snapshot,
    isLoading,
    error,
    refetch,
  } = useQuery({
    queryKey: ['resourceUsageSnapshot'],
    queryFn: invokeResourceSnapshot,
    enabled: IS_TAURI, // self-process sampling is only meaningful in the desktop app
    refetchInterval: 30_000,
  })

  return (
    <Card id="section-self-resource" variant="default" padding="lg">
      <div className="flex items-center justify-between gap-4">
        <div>
          <CardTitle>{t('metrics.selfUsageTitle', 'Agent resource usage')}</CardTitle>
          <p className="mt-1 text-content-tertiary text-xs">
            {t('metrics.selfUsageHint', "The desktop agent's own memory and CPU against its provisional budget.")}
          </p>
        </div>
        {IS_TAURI && (
          <Button variant="secondary" size="sm" onClick={() => refetch()} disabled={isLoading}>
            {isLoading ? <Spinner size="sm" /> : t('common.refresh', 'Refresh')}
          </Button>
        )}
      </div>

      {!IS_TAURI ? (
        <p className="mt-4 text-content-tertiary text-sm">
          {t('metrics.selfUsageDesktopOnly', 'Available in the desktop app.')}
        </p>
      ) : error ? (
        <Alert variant="error" title={t('metrics.snapshotError', 'Failed to sample resource usage')} className="mt-4">
          <div className="flex items-center justify-between gap-4">
            <span>{error instanceof Error ? error.message : String(error)}</span>
            <button type="button" className="text-sm underline hover:no-underline" onClick={() => refetch()}>
              {t('common.retry', 'Retry')}
            </button>
          </div>
        </Alert>
      ) : isLoading || !snapshot ? (
        <div className="mt-4 flex justify-center py-6">
          <Spinner size="sm" />
        </div>
      ) : (
        <div className="mt-4 space-y-1">
          <MetricRow
            label={t('metrics.selfRss', 'Memory (RSS)')}
            measured={snapshot.measured}
            value={formatBytes(snapshot.rss_bytes)}
            budget={formatBytes(snapshot.rss_budget_bytes)}
            withinBudget={snapshot.rss_within_budget}
          />
          <MetricRow
            label={t('metrics.selfCpu', 'CPU')}
            measured={snapshot.measured}
            value={formatPercent(snapshot.cpu_percent)}
            budget={formatPercent(snapshot.cpu_budget_percent)}
            withinBudget={snapshot.cpu_within_budget}
          />
          {!snapshot.measured && (
            <p className="pt-2 text-content-tertiary text-xs">
              {t('metrics.notMeasuredHint', 'Resource usage could not be measured on this platform.')}
            </p>
          )}
        </div>
      )}
    </Card>
  )
}
