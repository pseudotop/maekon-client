import { Target } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Card, CardContent, Skeleton } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { colors, iconSize, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { CoachingOutletContext } from './CoachingLayout'

export default function GoalsSection() {
  const { t } = useTranslation()
  const { goals, goalsLoading } = useTypedOutletContext<CoachingOutletContext>('Coaching')

  return (
    <section className="mb-6">
      <h2 className={cn(typography.h3, 'mb-3')}>{t('coaching.goalsTitle', "Today's Goals")}</h2>
      {goalsLoading ? (
        <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
          {[1, 2, 3, 4].map((i) => (
            <Skeleton key={i} className="h-20 w-full" />
          ))}
        </div>
      ) : goals && goals.length > 0 ? (
        <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
          {goals.map((g) => (
            <Card key={g.regime_label} variant="default" padding="sm">
              <CardContent>
                <div className="flex items-center gap-2">
                  <Target className={`${iconSize.base}`} style={{ color: g.display_color }} />
                  <span className={`text-sm ${typography.weight.medium}`}>{g.regime_label}</span>
                </div>
                <div className="mt-2 flex items-baseline gap-1">
                  <span className={`text-2xl ${typography.weight.bold}`}>{g.percentage ?? 0}%</span>
                  <span className="text-content-secondary text-xs">
                    {g.current_minutes ?? 0}/{g.target_minutes ?? 0}m
                  </span>
                </div>
                <div className="mt-1 h-1.5 overflow-hidden rounded-full bg-surface-muted">
                  <div
                    className={`h-full rounded-full ${motion.all} `.trim()}
                    style={{
                      width: `${Math.min(g.percentage ?? 0, 100)}%`,
                      backgroundColor: g.display_color,
                    }}
                  />
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      ) : (
        <Card variant="default" padding="md">
          <div className="flex flex-col items-center justify-center py-6 text-center">
            <div className="mb-3 flex h-12 w-12 items-center justify-center rounded-full bg-surface-elevated">
              <Target className="h-6 w-6 text-content-muted" />
            </div>
            <p className={cn(`text-sm ${typography.weight.medium}`, colors.text.primary)}>
              {t('coaching.noGoals', 'No goals configured.')}
            </p>
            <p className={cn('mt-1 text-xs', colors.text.secondary)}>
              {t('coaching.noGoalsHint', 'Goals will appear here once regime tracking is active.')}
            </p>
          </div>
        </Card>
      )}
    </section>
  )
}
