/**
 * Focus sessions section — work sessions list with pagination.
 */

import { useQuery } from '@tanstack/react-query'
import { Laptop } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchWorkSessions } from '../../api/client'
import { Badge } from '../../components/ui/Badge'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { useTypedOutletContext } from '../../routes'
import { iconSize, motion, typography } from '../../styles/tokens'
import { formatDuration } from '../../utils/formatters'
import type { FocusContext } from './FocusLayout'

const CATEGORY_COLORS: Record<string, string> = {
  Development: 'bg-brand-signal/80',
  Communication: 'bg-brand-signal/60',
  Documentation: 'bg-brand-signal/40',
  Browser: 'bg-brand-signal/70',
  Design: 'bg-brand-signal/50',
  Media: 'bg-brand-signal/30',
  System: 'bg-surface-muted',
  Other: 'bg-status-disconnected',
}

export default function SessionsSection() {
  const { t } = useTranslation()
  const { dateRange } = useTypedOutletContext<FocusContext>('Focus')
  const [sessionLimit, setSessionLimit] = useState(10)

  const { data: sessions = [] } = useQuery({
    queryKey: ['workSessions', dateRange.from.toISOString(), dateRange.to.toISOString()],
    queryFn: () => fetchWorkSessions(dateRange.from.toISOString(), dateRange.to.toISOString()),
  })

  return (
    <Card id="section-sessions">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Laptop className={`${iconSize.md}`} />
          {t('focus.sessions')}
        </CardTitle>
      </CardHeader>
      <CardContent>
        {sessions.length > 0 ? (
          <div className="max-h-80 space-y-3 overflow-y-auto">
            {sessions.slice(0, sessionLimit).map((session) => (
              <div key={session.id} className="flex items-center justify-between rounded-lg bg-surface-inset p-3">
                <div className="flex items-center gap-3">
                  <div
                    className={`${iconSize.xs} rounded-full ${CATEGORY_COLORS[session.category] || CATEGORY_COLORS.Other}`}
                  />
                  <div>
                    <p className={`${typography.weight.medium} text-content text-sm`}>{session.primary_app}</p>
                    <p className="text-content-tertiary text-xs">{session.category}</p>
                  </div>
                </div>
                <div className="text-right">
                  <p className={`${typography.weight.medium} text-content text-sm`}>
                    {formatDuration(session.duration_secs, true)}
                  </p>
                  <Badge color={session.state === 'active' ? 'success' : 'default'} size="sm">
                    {session.state === 'active' ? t('focus.active') : t('focus.completed')}
                  </Badge>
                </div>
              </div>
            ))}
            {sessions.length > sessionLimit && (
              <button
                type="button"
                onClick={() => setSessionLimit((limit) => limit + 10)}
                className={`mt-2 w-full rounded-lg py-2 ${typography.weight.medium} text-content-secondary text-sm ${motion.colors} hover:bg-surface-muted`}
              >
                {t('focus.loadMoreRemaining', { count: sessions.length - sessionLimit })}
              </button>
            )}
          </div>
        ) : (
          <p className="py-8 text-center text-content-tertiary">{t('common.noData')}</p>
        )}
      </CardContent>
    </Card>
  )
}
