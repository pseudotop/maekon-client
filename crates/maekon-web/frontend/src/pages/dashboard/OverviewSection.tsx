/**
 * Overview — the Dashboard's first screen (#13735).
 *
 * Top to bottom: a one-line summary of the day, what to do next, and four key
 * numbers. Everything comes from data the client already has: the daily
 * summary, the heuristic daily digest (no AI connection needed), and the
 * durable task store. CPU, memory and process data live on their own page
 * (`/monitoring`, linked from Support).
 *
 * Owns the Dashboard empty state. Lives here (not DashboardLayout) so that
 * the layout can always render <Outlet>, letting RouteRenderer's
 * <Navigate to="overview" replace /> index redirect fire on `/` → `/overview`
 * even when no data has been captured yet. Same empty-state-in-child pattern
 * AuditLayout adopted after the 2026-04-08 routing.spec regression.
 */

import { useQuery } from '@tanstack/react-query'
import { BarChart3, Brain, CalendarX, Clock, ListChecks, Monitor, Moon, Shuffle, Sun } from 'lucide-react'
import { useId } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { fetchDailyDigest } from '../../api/client'
import { hasCapturedActivity } from '../../components/CaptureProcessingStatus'
import StatCard, { statCardGrid } from '../../components/StatCard'
import TodaySummary from '../../components/TodaySummary'
import { Alert, Button, EmptyState, GuidanceEmptyState, Skeleton } from '../../components/ui'
import { useCaptureStatus } from '../../hooks/useCaptureStatus'
import { useTypedOutletContext } from '../../routes'
import { iconSize } from '../../styles/tokens'
import { formatDuration } from '../../utils/formatters'
import { parseLocalCalendarDate } from '../../utils/localDate'
import type { DashboardContext } from './DashboardLayout'
import {
  type DashboardFirstStepAction,
  resolveDashboardEmptyState,
  resolveDashboardFirstStep,
} from './dashboardEmptyState'
import OverviewNextActions from './NextActionsCard'
import { resolveTodayHeadline, resolveTodayNumbers, type TodayNumbers } from './todayDigest'

/** Stands in for a number the digest has not produced yet. */
const NO_VALUE = '—'

type DigestState = 'loading' | 'error' | 'ready'

function firstStepLabel(t: (key: string) => string, action: DashboardFirstStepAction): string {
  switch (action) {
    case 'reviewConsent':
      return t('dashboard.firstRun.action.reviewConsent')
    case 'resumeCapture':
      return t('dashboard.firstRun.action.resumeCapture')
    case 'checkStatus':
      return t('dashboard.firstRun.action.checkStatus')
  }
}

/** No activity today: say what is going on with capture and what to do next. */
function FirstRunEmptyState() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const captureState = resolveDashboardEmptyState(useCaptureStatus())
  const firstStep = resolveDashboardFirstStep(captureState)

  return (
    <GuidanceEmptyState
      data-testid="overview-first-run"
      data-capture-state={captureState}
      className="py-8"
      icon={<Monitor className={iconSize.hero} aria-hidden="true" />}
      title={t(`emptyState.dashboard.${captureState}.title`)}
      description={t(`emptyState.dashboard.${captureState}.description`)}
      guidance={[
        {
          icon: <Sun className={iconSize.sm} aria-hidden="true" />,
          title: t('dashboard.todaySummary.title'),
          description: t('dashboard.firstRun.guideSummary'),
        },
        {
          icon: <ListChecks className={iconSize.sm} aria-hidden="true" />,
          title: t('dashboard.nextActions.title'),
          description: t('dashboard.firstRun.guideNextActions'),
        },
        {
          icon: <BarChart3 className={iconSize.sm} aria-hidden="true" />,
          title: t('dashboard.keyNumbers.title'),
          description: t('dashboard.firstRun.guideKeyNumbers'),
        },
      ]}
      primaryAction={
        firstStep ? { label: firstStepLabel(t, firstStep.action), onClick: () => navigate(firstStep.path) } : undefined
      }
    />
  )
}

/** A past day with nothing captured is not a first run; just say so. */
function NoActivityForDate({ date }: { date: string }) {
  const { t, i18n } = useTranslation()
  const parsed = parseLocalCalendarDate(date)
  const label = parsed ? parsed.toLocaleDateString(i18n.resolvedLanguage, { month: 'long', day: 'numeric' }) : date

  return (
    <div data-testid="overview-no-activity-for-date">
      <EmptyState
        icon={<CalendarX className={iconSize.hero} aria-hidden="true" />}
        title={t('dashboard.noActivityForDate.title')}
        description={t('dashboard.noActivityForDate.description', { date: label })}
      />
    </div>
  )
}

function SummaryErrorAlert({ onRetry }: { onRetry: () => void }) {
  const { t } = useTranslation()
  return (
    <Alert variant="error" title={t('dashboard.summaryError.title')} data-testid="overview-summary-error">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <span>{t('dashboard.summaryError.description')}</span>
        <Button variant="secondary" size="sm" onClick={onRetry}>
          {t('common.retry')}
        </Button>
      </div>
    </Alert>
  )
}

function OverviewSkeleton() {
  return (
    <div data-testid="overview-loading" aria-busy="true" className="space-y-6">
      <Skeleton className="h-24 w-full" />
      <Skeleton className="h-24 w-full" />
      <div className={statCardGrid}>
        {['active', 'deep-work', 'switches', 'idle'].map((id) => (
          <Skeleton key={id} className="h-20" />
        ))}
      </div>
    </div>
  )
}

function KeyNumbers({ numbers, digestState }: { numbers: TodayNumbers; digestState: DigestState }) {
  const { t, i18n } = useTranslation()
  const headingId = useId()
  // Why a digest number is missing. Nothing while the digest is still loading.
  const missingDetail =
    digestState === 'error'
      ? t('dashboard.keyNumbers.unavailable')
      : digestState === 'ready'
        ? t('dashboard.keyNumbers.pendingAnalysis')
        : undefined

  return (
    <section aria-labelledby={headingId} data-testid="overview-key-numbers">
      <h2 id={headingId} className="sr-only">
        {t('dashboard.keyNumbers.title')}
      </h2>
      <div className={statCardGrid}>
        {/* Active time appears once on this screen, always in the short format. */}
        <StatCard
          data-testid="metric-card-active-time"
          title={t('dashboard.activeTime')}
          value={formatDuration(numbers.activeSecs, true)}
          icon={<Clock className={iconSize.md} />}
        />
        <StatCard
          data-testid="metric-card-deep-work"
          title={t('dashboard.keyNumbers.deepWork')}
          value={numbers.deepWorkSecs === null ? NO_VALUE : formatDuration(numbers.deepWorkSecs, true)}
          detail={numbers.deepWorkSecs === null ? missingDetail : undefined}
          icon={<Brain className={iconSize.md} />}
        />
        <StatCard
          data-testid="metric-card-context-switches"
          title={t('dashboard.keyNumbers.contextSwitches')}
          value={
            numbers.contextSwitches === null ? NO_VALUE : numbers.contextSwitches.toLocaleString(i18n.resolvedLanguage)
          }
          detail={numbers.contextSwitches === null ? missingDetail : undefined}
          icon={<Shuffle className={iconSize.md} />}
        />
        <StatCard
          data-testid="metric-card-idle-time"
          title={t('dashboard.idleTime')}
          value={formatDuration(numbers.idleSecs, true)}
          icon={<Moon className={iconSize.md} />}
        />
      </div>
    </section>
  )
}

export default function OverviewSection() {
  const { summary, summaryLoading, summaryError, refetchSummary, selectedDate, isToday, isWidgetVisible } =
    useTypedOutletContext<DashboardContext>('Dashboard')

  // Shares the Day view's cache entry. One retry keeps a failing digest from
  // holding the summary skeleton for the default back-off.
  const digestQuery = useQuery({
    queryKey: ['dashboard-day', selectedDate],
    queryFn: () => fetchDailyDigest(selectedDate),
    retry: 1,
  })

  if (summaryLoading) return <OverviewSkeleton />
  if (summaryError && !summary) return <SummaryErrorAlert onRetry={refetchSummary} />

  // Realtime CPU samples are not activity: an open stream with nothing
  // captured is still an empty first screen, not a row of zeros.
  if (!hasCapturedActivity(summary)) {
    return (
      <>
        {isToday ? <FirstRunEmptyState /> : <NoActivityForDate date={selectedDate} />}
        {isWidgetVisible('overview.next-actions') && <OverviewNextActions hideWhenEmpty />}
      </>
    )
  }

  const digest = digestQuery.data
  const digestState: DigestState = digestQuery.isLoading ? 'loading' : digestQuery.isError ? 'error' : 'ready'

  return (
    <>
      {isWidgetVisible('overview.today-summary') && (
        <TodaySummary
          headline={resolveTodayHeadline(summary, digest)}
          pastDate={isToday ? null : selectedDate}
          loading={digestState === 'loading'}
        />
      )}
      {isWidgetVisible('overview.next-actions') && <OverviewNextActions />}
      {isWidgetVisible('overview.stat-cards') && (
        <KeyNumbers numbers={resolveTodayNumbers(summary, digest)} digestState={digestState} />
      )}
    </>
  )
}
