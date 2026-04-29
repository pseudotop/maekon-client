import { Trash2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Badge, Button, Card, Skeleton } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { RecalibrationOutletContext } from './RecalibrationLayout'
import { formatDateTime, getActionLabel } from './RecalibrationLayout'

export default function OverridesSection() {
  const { t } = useTranslation()
  const { overrides, overridesLoading, deleteOverride } =
    useTypedOutletContext<RecalibrationOutletContext>('Recalibration')

  return (
    <Card padding="md">
      <h2 className={cn(typography.h3, colors.text.primary, 'mb-4')}>{t('recalibration.overrideHistory')}</h2>

      {overridesLoading && (
        <div className="space-y-2">
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-full" />
        </div>
      )}

      {!overridesLoading && (!overrides || overrides.length === 0) && (
        <p className={cn('py-4 text-center text-sm', colors.text.secondary)}>{t('recalibration.noOverrides')}</p>
      )}

      {!overridesLoading && overrides && overrides.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[600px] text-left text-sm">
            <thead>
              <tr className={cn('border-DEFAULT border-b', colors.text.tertiary)}>
                <th className={`pr-4 pb-2 ${typography.weight.medium}`}>Segment</th>
                <th className={`pr-4 pb-2 ${typography.weight.medium}`}>Original</th>
                <th className={`pr-4 pb-2 ${typography.weight.medium}`}>Action</th>
                <th className={`pr-4 pb-2 ${typography.weight.medium}`}>Created</th>
                <th className={`pb-2 ${typography.weight.medium}`} />
              </tr>
            </thead>
            <tbody>
              {overrides.map((override) => (
                <tr key={override.override_id} className="border-DEFAULT border-b last:border-b-0">
                  <td className={cn(`py-2 pr-4 ${typography.family.mono} text-xs`, colors.text.secondary)}>
                    {override.segment_id.slice(0, 8)}...
                  </td>
                  <td className={cn('py-2 pr-4 text-xs', colors.text.secondary)}>
                    {override.original_regime_id ?? '-'}
                  </td>
                  <td className="py-2 pr-4">
                    <Badge color="info" size="sm">
                      {getActionLabel(override, t)}
                    </Badge>
                  </td>
                  <td className={cn('py-2 pr-4 text-xs', colors.text.tertiary)}>
                    {formatDateTime(override.created_at)}
                  </td>
                  <td className="py-2">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => deleteOverride.mutate(override.override_id)}
                      disabled={deleteOverride.isPending}
                      aria-label={t('recalibration.undo')}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      <span className="ml-1">{t('recalibration.undo')}</span>
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  )
}
