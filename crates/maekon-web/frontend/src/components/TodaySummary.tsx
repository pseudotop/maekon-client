/**
 * Today's one-line summary — the first block of the Dashboard overview (#13735).
 *
 * Renders a headline resolved by `resolveTodayHeadline` and always says where
 * it came from: the AI daily narrative (with its provider class) or the
 * heuristic digest that works without any AI connection.
 */

import { useTranslation } from 'react-i18next'
import type { TodayHeadline } from '../pages/dashboard/todayDigest'
import { colors, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { formatDuration } from '../utils/formatters'
import { parseLocalCalendarDate } from '../utils/localDate'
import { Card, CardTitle, Skeleton } from './ui'

export interface TodaySummaryProps {
  headline: TodayHeadline
  /** Local calendar date (`YYYY-MM-DD`) when the summary is not for today. */
  pastDate?: string | null
  /** True while the digest that can refine the headline is still loading. */
  loading?: boolean
}

function HeadlineText({ headline }: { headline: Exclude<TodayHeadline, { kind: 'none' }> }) {
  const { t } = useTranslation()
  if (headline.kind === 'ai') return <>{headline.text}</>

  switch (headline.variant) {
    case 'focus':
      return (
        <>
          {t('dashboard.todaySummary.headline.focus', {
            app: headline.topApp,
            longestFocus: formatDuration(headline.longestFocusSecs, true),
          })}
        </>
      )
    case 'app':
      return <>{t('dashboard.todaySummary.headline.app', { app: headline.topApp })}</>
    default:
      return <>{t('dashboard.todaySummary.headline.activity')}</>
  }
}

function ProvenanceBadge({ headline }: { headline: Exclude<TodayHeadline, { kind: 'none' }> }) {
  const { t, i18n } = useTranslation()
  if (headline.kind === 'ai') {
    return (
      <span className="flex flex-wrap items-center gap-2">
        <span
          data-testid="today-summary-provenance"
          data-provenance="ai"
          className="rounded-full bg-brand-signal/10 px-2 py-1 text-brand-text text-xs"
        >
          {t('summaryProvenance.aiDailyNarrative')} · {t(`summaryProvenance.provider.${headline.providerClass}`)}
        </span>
        {headline.generatedAt && (
          <time className="text-content-tertiary text-xs" dateTime={headline.generatedAt}>
            {new Date(headline.generatedAt).toLocaleString(i18n.resolvedLanguage)}
          </time>
        )}
      </span>
    )
  }
  return (
    <span
      data-testid="today-summary-provenance"
      data-provenance="heuristic"
      className="rounded-full bg-surface-muted px-2 py-1 text-content-secondary text-xs"
    >
      {t('summaryProvenance.digest.heuristic')}
    </span>
  )
}

export default function TodaySummary({ headline, pastDate, loading = false }: TodaySummaryProps) {
  const { t, i18n } = useTranslation()

  if (headline.kind === 'none' && !loading) return null

  const parsedPastDate = pastDate ? parseLocalCalendarDate(pastDate) : null
  const title = parsedPastDate
    ? t('dashboard.todaySummary.titleForDate', {
        date: parsedPastDate.toLocaleDateString(i18n.resolvedLanguage, { month: 'long', day: 'numeric' }),
      })
    : t('dashboard.todaySummary.title')

  return (
    <Card variant="default" padding="md" data-testid="today-summary" aria-busy={loading || undefined}>
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <CardTitle>{title}</CardTitle>
        {!loading && headline.kind !== 'none' && <ProvenanceBadge headline={headline} />}
      </div>
      {loading || headline.kind === 'none' ? (
        <Skeleton className="h-6 w-3/4" />
      ) : (
        <p
          data-testid="today-summary-headline"
          className={cn(typography.h4, colors.text.primary, 'whitespace-pre-line leading-relaxed')}
        >
          <HeadlineText headline={headline} />
        </p>
      )}
    </Card>
  )
}
