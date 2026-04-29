/**
 * RecalibrationLayout — bulk regime correction with date-range filtering,
 * segment list, override history, and recluster trigger.
 */
import { useQuery } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { useCallback, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet } from 'react-router-dom'
import { fetchDailyDigest } from '../../api/client'
import type {
  CreateOverrideRequest,
  DailyDigestResponse,
  DailyDigestSegment,
  RegimeOverride,
} from '../../api/contracts'
import DateRangePicker from '../../components/DateRangePicker'
import { Button, Card, Spinner } from '../../components/ui'
import { useCreateOverride, useDeleteOverride, useOverrides, useRecluster } from '../../hooks/useRecalibration'
import { colors, iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

// Default regime options
const REGIME_OPTIONS = [
  { id: 'deep-work', label: 'Deep Work' },
  { id: 'communication', label: 'Communication' },
  { id: 'meeting', label: 'Meeting' },
  { id: 'break', label: 'Break' },
  { id: 'admin', label: 'Admin' },
]

export function formatTime(iso: string): string {
  try {
    return new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false })
  } catch {
    return iso.slice(11, 16)
  }
}

export function formatDateTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString([], {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    })
  } catch {
    return iso
  }
}

export function getActionLabel(override: RegimeOverride, t: (key: string) => string): string {
  switch (override.user_action.type) {
    case 'MARK_AS_PERSONAL_TIME':
      return t('recalibration.personalTime')
    case 'MARK_AS_NOISE':
      return t('recalibration.personalTime')
    case 'REASSIGN_REGIME': {
      const action = override.user_action as { type: 'REASSIGN_REGIME'; target_regime_id: string }
      const regime = REGIME_OPTIONS.find((r) => r.id === action.target_regime_id)
      return regime?.label ?? action.target_regime_id
    }
    default:
      return t('recalibration.overridden')
  }
}

export { REGIME_OPTIONS }

export interface RecalibrationOutletContext {
  segments: DailyDigestSegment[]
  segmentsLoading: boolean
  overrides: RegimeOverride[] | undefined
  overridesLoading: boolean
  overrideMap: Map<string, RegimeOverride>
  createOverride: ReturnType<typeof useCreateOverride>
  deleteOverride: ReturnType<typeof useDeleteOverride>
  handleMarkAsPersonal: (seg: DailyDigestSegment) => void
  handleReassign: (seg: DailyDigestSegment, targetRegimeId: string) => void
}

export default function RecalibrationLayout() {
  const { t } = useTranslation()
  const [from, setFrom] = useState<string | undefined>()
  const [to, setTo] = useState<string | undefined>()

  const handleRangeChange = useCallback((newFrom: string | undefined, newTo: string | undefined) => {
    setFrom(newFrom)
    setTo(newTo)
  }, [])

  // Fetch overrides
  const { data: overrides, isLoading: overridesLoading } = useOverrides(from, to)

  // Fetch segments from daily digest for the date range
  const { data: digestData, isLoading: segmentsLoading } = useQuery<DailyDigestResponse>({
    queryKey: ['recalibration-segments', from, to],
    queryFn: async () => {
      // Use the date part only for the API call
      const dateStr = from ? from.split('T')[0] : new Date().toISOString().split('T')[0]
      return fetchDailyDigest(dateStr)
    },
    enabled: !!from,
  })

  // Mutations
  const createOverride = useCreateOverride()
  const deleteOverride = useDeleteOverride()
  const recluster = useRecluster()

  // Build override lookup
  const overrideMap = useMemo(() => {
    const map = new Map<string, RegimeOverride>()
    if (overrides) {
      for (const o of overrides) {
        map.set(o.segment_id, o)
      }
    }
    return map
  }, [overrides])

  const segments = digestData?.timeline ?? []

  const handleMarkAsPersonal = (seg: DailyDigestSegment) => {
    const req: CreateOverrideRequest = {
      segment_id: seg.segment_id,
      original_regime_id: seg.regime_id,
      action: { type: 'MARK_AS_PERSONAL_TIME', from: seg.start_time, to: seg.end_time },
    }
    createOverride.mutate(req)
  }

  const handleReassign = (seg: DailyDigestSegment, targetRegimeId: string) => {
    const req: CreateOverrideRequest = {
      segment_id: seg.segment_id,
      original_regime_id: seg.regime_id,
      action: { type: 'REASSIGN_REGIME', target_regime_id: targetRegimeId },
    }
    createOverride.mutate(req)
  }

  const handleBulkMarkPersonal = () => {
    for (const seg of segments) {
      if (!overrideMap.has(seg.segment_id)) {
        handleMarkAsPersonal(seg)
      }
    }
  }

  const ctx: RecalibrationOutletContext = {
    segments,
    segmentsLoading,
    overrides,
    overridesLoading,
    overrideMap,
    createOverride,
    deleteOverride,
    handleMarkAsPersonal,
    handleReassign,
  }

  return (
    <div className="min-h-full space-y-6 p-6">
      {/* Header */}
      <div className="flex flex-col justify-between gap-4 md:flex-row md:items-center">
        <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('recalibration.title')}</h1>

        <Button variant="primary" size="sm" onClick={() => recluster.mutate()} disabled={recluster.isPending}>
          {recluster.isPending ? (
            <>
              <Spinner size="sm" />
              <span className="ml-2">{t('recalibration.reclustering')}</span>
            </>
          ) : (
            <>
              <RefreshCw className={`mr-2 ${iconSize.base}`} />
              {t('recalibration.triggerRecluster')}
            </>
          )}
        </Button>
      </div>

      {/* Controls */}
      <Card padding="md">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <DateRangePicker onRangeChange={handleRangeChange} />
          <Button
            variant="secondary"
            size="sm"
            onClick={handleBulkMarkPersonal}
            disabled={segments.length === 0 || createOverride.isPending}
          >
            {t('recalibration.markRangePersonal')}
          </Button>
        </div>
      </Card>

      <Outlet context={ctx} />
    </div>
  )
}
