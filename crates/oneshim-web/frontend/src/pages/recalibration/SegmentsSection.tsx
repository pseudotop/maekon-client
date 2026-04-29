import { RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Badge, Button, Card, EmptyState, Select, Skeleton } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { RecalibrationOutletContext } from './RecalibrationLayout'
import { formatTime, getActionLabel, REGIME_OPTIONS } from './RecalibrationLayout'

export default function SegmentsSection() {
  const { t } = useTranslation()
  const { segments, segmentsLoading, overrideMap, createOverride, handleMarkAsPersonal, handleReassign } =
    useTypedOutletContext<RecalibrationOutletContext>('Recalibration')

  return (
    <Card padding="md">
      <h2 className={cn(typography.h3, colors.text.primary, 'mb-4')}>Segments</h2>

      {segmentsLoading && (
        <div className="space-y-2">
          <Skeleton className="h-12 w-full" />
          <Skeleton className="h-12 w-full" />
          <Skeleton className="h-12 w-full" />
        </div>
      )}

      {!segmentsLoading && segments.length === 0 && (
        <EmptyState
          icon={<RefreshCw className="h-8 w-8" />}
          title={t('recalibration.noSegments')}
          description={t(
            'recalibration.noSegmentsDescription',
            'Try selecting a different date range to find activity segments.',
          )}
        />
      )}

      {!segmentsLoading && segments.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[600px] text-left text-sm">
            <thead>
              <tr className={cn('border-DEFAULT border-b', colors.text.tertiary)}>
                <th className={`pr-4 pb-2 ${typography.weight.medium}`}>Time</th>
                <th className={`pr-4 pb-2 ${typography.weight.medium}`}>Duration</th>
                <th className={`pr-4 pb-2 ${typography.weight.medium}`}>Regime</th>
                <th className={`pr-4 pb-2 ${typography.weight.medium}`}>App</th>
                <th className={`pb-2 ${typography.weight.medium}`}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {segments.map((seg) => {
                const override = overrideMap.get(seg.segment_id)
                const isOverridden = !!override
                return (
                  <tr key={seg.segment_id} className="border-DEFAULT border-b last:border-b-0">
                    <td className={cn('py-2 pr-4 text-xs', colors.text.secondary)}>
                      {formatTime(seg.start_time)} - {formatTime(seg.end_time)}
                    </td>
                    <td className={cn('py-2 pr-4 text-xs', colors.text.secondary)}>{seg.duration_mins}m</td>
                    <td className="py-2 pr-4">
                      <div className="flex items-center gap-2">
                        <span
                          className="inline-block h-2.5 w-2.5 rounded-full"
                          style={{ backgroundColor: seg.regime_color }}
                        />
                        <span className={cn('text-xs', colors.text.primary, isOverridden ? 'line-through' : '')}>
                          {seg.regime_label}
                        </span>
                        {override && (
                          <Badge color="warning" size="sm">
                            {getActionLabel(override, t)}
                          </Badge>
                        )}
                      </div>
                    </td>
                    <td className={cn('py-2 pr-4 text-xs', colors.text.secondary)}>{seg.dominant_app}</td>
                    <td className="py-2">
                      <div className="flex items-center gap-2">
                        {!isOverridden && (
                          <>
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => handleMarkAsPersonal(seg)}
                              disabled={createOverride.isPending}
                            >
                              {t('recalibration.markAsPersonalTime')}
                            </Button>
                            <Select
                              selectSize="sm"
                              defaultValue=""
                              onChange={(e) => {
                                if (e.target.value) handleReassign(seg, e.target.value)
                              }}
                              className="w-36"
                            >
                              <option value="" disabled>
                                {t('recalibration.changeRegimeTo')}
                              </option>
                              {REGIME_OPTIONS.filter((r) => r.id !== seg.regime_id).map((r) => (
                                <option key={r.id} value={r.id}>
                                  {r.label}
                                </option>
                              ))}
                            </Select>
                          </>
                        )}
                      </div>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  )
}
