/**
 * Focus interruptions section — interruptions list with pagination.
 */

import { useQuery } from '@tanstack/react-query'
import { ArrowRightLeft } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchInterruptions } from '../../api/client'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { useTypedOutletContext } from '../../routes'
import { iconSize, motion, typography } from '../../styles/tokens'
import { formatDuration } from '../../utils/formatters'
import type { FocusContext } from './FocusLayout'

export default function InterruptionsSection() {
  const { t } = useTranslation()
  const { dateRange } = useTypedOutletContext<FocusContext>('Focus')
  const [interruptionLimit, setInterruptionLimit] = useState(10)

  const { data: interruptions = [] } = useQuery({
    queryKey: ['interruptions', dateRange.from.toISOString(), dateRange.to.toISOString()],
    queryFn: () => fetchInterruptions(dateRange.from.toISOString(), dateRange.to.toISOString()),
  })

  return (
    <Card id="section-interruptions">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <ArrowRightLeft className={`${iconSize.md}`} />
          {t('focus.interruptionList')}
        </CardTitle>
      </CardHeader>
      <CardContent>
        {interruptions.length > 0 ? (
          <div className="max-h-80 space-y-3 overflow-y-auto">
            {interruptions.slice(0, interruptionLimit).map((int) => (
              <div key={int.id} className="flex items-center justify-between rounded-lg bg-surface-inset p-3">
                <div className="flex items-center gap-2 text-sm">
                  <span className={`max-w-[80px] truncate ${typography.weight.medium} text-content`}>
                    {int.from_app}
                  </span>
                  <ArrowRightLeft className={`${iconSize.base} text-content-muted`} />
                  <span className={`max-w-[80px] truncate ${typography.weight.medium} text-content`}>{int.to_app}</span>
                </div>
                <div className="text-right">
                  <p className="text-content-tertiary text-xs">
                    {new Date(int.interrupted_at).toLocaleTimeString([], {
                      hour: '2-digit',
                      minute: '2-digit',
                    })}
                  </p>
                  {int.duration_secs && (
                    <p className="text-content-tertiary text-xs">{formatDuration(int.duration_secs, true)}</p>
                  )}
                </div>
              </div>
            ))}
            {interruptions.length > interruptionLimit && (
              <button
                type="button"
                onClick={() => setInterruptionLimit((limit) => limit + 10)}
                className={`mt-2 w-full rounded-lg py-2 ${typography.weight.medium} text-content-secondary text-sm ${motion.colors} hover:bg-surface-muted`}
              >
                {t('focus.loadMoreRemaining', { count: interruptions.length - interruptionLimit })}
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
