import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { Outlet } from 'react-router-dom'
import { type AuditEntry, fetchAuditLogs, fetchAutomationStats } from '../../api/client'
import type { AutomationStats } from '../../api/contracts'
import { ListSkeleton, Skeleton, StatCardsSkeleton } from '../../components/ui'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

export interface AuditOutletContext {
  auditLogs: AuditEntry[] | undefined
  logsLoading: boolean
  stats: AutomationStats | undefined
  statsLoading: boolean
}

export default function AuditLayout() {
  const { t } = useTranslation()

  const { data: auditLogs, isLoading: logsLoading } = useQuery({
    queryKey: ['auditLogPage', ''],
    queryFn: () => fetchAuditLogs(100, undefined),
    refetchInterval: 10_000,
  })

  const { data: stats, isLoading: statsLoading } = useQuery({
    queryKey: ['automationStats'],
    queryFn: fetchAutomationStats,
    refetchInterval: 10_000,
  })

  if (logsLoading || statsLoading) {
    return (
      <div className="min-h-full space-y-6 p-6">
        <Skeleton className="h-8 w-48" />
        <StatCardsSkeleton count={5} />
        <Skeleton className="h-10 w-full" />
        <ListSkeleton rows={8} />
      </div>
    )
  }

  // The empty-state UX is owned by SummarySection so that AuditLayout can keep
  // rendering <Outlet> on every path. An earlier revision short-circuited the
  // layout with EmptyState when no audit data existed, which suppressed the
  // index <Navigate to="summary" replace /> emitted by RouteRenderer and left
  // /audit stuck without redirecting to /audit/summary (caught by routing.spec).
  const ctx: AuditOutletContext = { auditLogs, logsLoading, stats, statsLoading }

  return (
    <div className="min-h-full space-y-6 p-6">
      <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('auditLog.title')}</h1>
      <Outlet context={ctx} />
    </div>
  )
}
