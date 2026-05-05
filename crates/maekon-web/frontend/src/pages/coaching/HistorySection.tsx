import { MessageCircle, TrendingUp } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Card, CardContent, Skeleton } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { colors, iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { CoachingOutletContext } from './CoachingLayout'

export default function HistorySection() {
  const { t } = useTranslation()
  const { history, histLoading } = useTypedOutletContext<CoachingOutletContext>('Coaching')

  return (
    <section>
      <h2 className={cn(typography.h3, 'mb-3')}>{t('coaching.recentEvents', 'Recent Events')}</h2>
      {histLoading ? (
        <div className="space-y-3">
          {[1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-16 w-full" />
          ))}
        </div>
      ) : history && history.length > 0 ? (
        <div className="space-y-3">
          {history.map((evt) => (
            <Card key={evt.event_id} variant="default" padding="sm">
              <CardContent>
                <div className="flex items-start justify-between">
                  <div className="flex items-center gap-2">
                    <MessageCircle className={`${iconSize.base} text-brand-text`} />
                    <span className={`rounded-md bg-surface-elevated px-2 py-0.5 text-xs ${typography.weight.medium}`}>
                      {evt.profile_name}
                    </span>
                    <span className="text-content-secondary text-xs">{evt.trigger_type}</span>
                  </div>
                  <span className="text-content-secondary text-xs">{new Date(evt.shown_at).toLocaleTimeString()}</span>
                </div>
                <p className="mt-1 text-content text-sm">{evt.personalized_message || evt.message_template}</p>
                {evt.feedback_type && (
                  <div className="mt-1 flex items-center gap-1">
                    <TrendingUp className={`${iconSize.xs}`} />
                    <span className="text-content-secondary text-xs">{evt.feedback_type}</span>
                  </div>
                )}
              </CardContent>
            </Card>
          ))}
        </div>
      ) : (
        <Card variant="default" padding="md">
          <div className="flex flex-col items-center justify-center py-6 text-center">
            <div className="mb-3 flex h-12 w-12 items-center justify-center rounded-full bg-surface-elevated">
              <MessageCircle className="h-6 w-6 text-content-muted" />
            </div>
            <p className={cn(`text-sm ${typography.weight.medium}`, colors.text.primary)}>
              {t('coaching.noEvents', 'No coaching events yet.')}
            </p>
            <p className={cn('mt-1 text-xs', colors.text.secondary)}>
              {t('coaching.noEventsHint', 'Coaching nudges will appear here as you work.')}
            </p>
          </div>
        </Card>
      )}
    </section>
  )
}
