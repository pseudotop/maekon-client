/**
 * Egress transparency browser — read-only "what left this device" audit panel
 * (T1.2, #7910).
 *
 * Backed by the erase-retained #4803 egress ledger via
 * `GET /api/privacy/egress-ledger`. Read-only by design: the ledger is
 * regulatory-compliance evidence retained across GDPR erasure, so this panel has
 * NO delete/edit affordance. The nine displayed fields are the ledger's own
 * columns, which contain no captured content / PII by construction (they record
 * *that* egress happened — byte counts, destination sink strings, disposition —
 * never *what* was sent). `capture_blocked` rows (destination `local.capture`,
 * byte/recipient 0) are prevented-capture evidence: a frame deliberately NOT
 * captured, rendered distinctly so byte 0 never reads as "0 bytes uploaded".
 */

import { useQuery } from '@tanstack/react-query'
import { ShieldCheck } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchEgressLedger } from '../../api/client'
import type { EgressLedgerEntry } from '../../api/contracts'
import { Alert, Badge, type BadgeProps, Card, CardTitle, EmptyState, Select, Spinner } from '../../components/ui'
import { typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { formatBytes, formatDateTime, formatNumber } from '../../utils/formatters'
import AiUsageCard from './AiUsageCard'

type RangePreset = '24h' | '7d' | '30d' | 'all'
type DispositionFilter = 'all' | 'uploaded' | 'blocked' | 'capture_blocked'

const RANGE_PRESETS: RangePreset[] = ['24h', '7d', '30d', 'all']
const DISPOSITION_FILTERS: DispositionFilter[] = ['all', 'uploaded', 'blocked', 'capture_blocked']

// Shared cell classes (design-token weights/family, not hardcoded font-*).
const HEADER_CELL = cn('py-2 pr-4', typography.weight.medium)
const HEADER_CELL_RIGHT = cn('py-2 pr-4 text-right', typography.weight.medium)
const MONO_CELL = cn('py-2 pr-4 text-content-secondary text-xs', typography.family.mono)

/** Recent-mode fetch cap for the "all" preset (matches the server-side cap). */
const ALL_LIMIT = 1000

/** Milliseconds per preset window; `all` has no window (recent mode). */
const RANGE_WINDOW_MS: Record<Exclude<RangePreset, 'all'>, number> = {
  '24h': 24 * 60 * 60 * 1000,
  '7d': 7 * 24 * 60 * 60 * 1000,
  '30d': 30 * 24 * 60 * 60 * 1000,
}

/** Badge color per disposition. capture_blocked is visually distinct (not egress). */
function dispositionBadgeColor(disposition: string): BadgeProps['color'] {
  switch (disposition) {
    case 'uploaded':
      return 'info'
    case 'blocked':
      return 'warning'
    case 'capture_blocked':
      return 'purple'
    default:
      return 'default'
  }
}

export default function EgressLedgerSection() {
  const { t } = useTranslation()
  const [range, setRange] = useState<RangePreset>('7d')
  const [disposition, setDisposition] = useState<DispositionFilter>('all')

  // Derive the from/to window for the query. `all` → recent mode (no range).
  const params = useMemo(() => {
    if (range === 'all') return { limit: ALL_LIMIT }
    const now = Date.now()
    return {
      from: new Date(now - RANGE_WINDOW_MS[range]).toISOString(),
      to: new Date(now).toISOString(),
    }
  }, [range])

  const { data, isLoading, isError, error } = useQuery({
    queryKey: ['egress-ledger', params],
    queryFn: () => fetchEgressLedger(params),
  })

  const rawEntries = data?.entries ?? []
  const filtered = useMemo(
    () => (disposition === 'all' ? rawEntries : rawEntries.filter((e) => e.disposition === disposition)),
    [rawEntries, disposition],
  )

  // Distinguish "the ledger is empty" (nothing has ever left this device) from
  // "no entries in the selected range / disposition filter".
  const ledgerLikelyEmpty = range === 'all' && disposition === 'all' && rawEntries.length === 0

  return (
    <>
      {/* #9466: spend + egress are two faces of one trust surface. */}
      <AiUsageCard />
      <Card id="section-egress" variant="default" padding="lg">
        <CardTitle className="mb-1">{t('privacy.egress.title')}</CardTitle>
        <p className="mb-4 text-content-secondary text-sm">{t('privacy.egress.description')}</p>

        {/* Filters */}
        <div className="mb-4 flex flex-wrap gap-4">
          <label className="flex flex-col gap-1 text-content-secondary text-xs">
            {t('privacy.egress.rangeLabel')}
            <Select
              aria-label={t('privacy.egress.rangeLabel')}
              value={range}
              selectSize="sm"
              className="w-40"
              onChange={(e) => setRange(e.target.value as RangePreset)}
            >
              {RANGE_PRESETS.map((preset) => (
                <option key={preset} value={preset}>
                  {t(`privacy.egress.range.${preset}`)}
                </option>
              ))}
            </Select>
          </label>
          <label className="flex flex-col gap-1 text-content-secondary text-xs">
            {t('privacy.egress.dispositionLabel')}
            <Select
              aria-label={t('privacy.egress.dispositionLabel')}
              value={disposition}
              selectSize="sm"
              className="w-48"
              onChange={(e) => setDisposition(e.target.value as DispositionFilter)}
            >
              {DISPOSITION_FILTERS.map((value) => (
                <option key={value} value={value}>
                  {t(`privacy.egress.disposition.${value}`)}
                </option>
              ))}
            </Select>
          </label>
        </div>

        {isLoading && (
          <div className="flex items-center justify-center py-12">
            <Spinner size="lg" className="text-brand-text" />
            <span className="ml-3 text-content-secondary text-sm">{t('common.loading')}</span>
          </div>
        )}

        {isError && (
          <Alert variant="error" title={t('privacy.egress.loadError')} className="mb-2">
            {(error as Error).message}
          </Alert>
        )}

        {!isLoading && !isError && filtered.length === 0 && (
          <EmptyState
            icon={<ShieldCheck size={28} aria-hidden="true" />}
            title={ledgerLikelyEmpty ? t('privacy.egress.empty.ledgerTitle') : t('privacy.egress.empty.rangeTitle')}
            description={ledgerLikelyEmpty ? t('privacy.egress.empty.ledgerDesc') : t('privacy.egress.empty.rangeDesc')}
          />
        )}

        {!isLoading && !isError && filtered.length > 0 && (
          <>
            <p className="mb-2 text-content-tertiary text-xs">
              {t('privacy.egress.entryCount', { count: filtered.length })}
            </p>
            <div className="overflow-x-auto">
              <table className="w-full text-left text-sm">
                <thead>
                  <tr className="border-muted border-b text-content-secondary text-xs">
                    <th className={HEADER_CELL}>{t('privacy.egress.col.time')}</th>
                    <th className={HEADER_CELL}>{t('privacy.egress.col.eventType')}</th>
                    <th className={HEADER_CELL}>{t('privacy.egress.col.destination')}</th>
                    <th className={HEADER_CELL}>{t('privacy.egress.col.disposition')}</th>
                    <th className={HEADER_CELL_RIGHT}>{t('privacy.egress.col.bytes')}</th>
                    <th className={HEADER_CELL_RIGHT}>{t('privacy.egress.col.recipients')}</th>
                    <th className={HEADER_CELL}>{t('privacy.egress.col.consent')}</th>
                  </tr>
                </thead>
                <tbody>
                  {filtered.map((entry) => (
                    <EgressRow key={entry.record_id} entry={entry} />
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
      </Card>
    </>
  )
}

function EgressRow({ entry }: { entry: EgressLedgerEntry }) {
  const { t } = useTranslation()
  const isCaptureBlocked = entry.disposition === 'capture_blocked'
  const badgeLabelKey = DISPOSITION_FILTERS.includes(entry.disposition as DispositionFilter)
    ? `privacy.egress.badge.${entry.disposition}`
    : null

  return (
    <tr className="border-muted/50 border-b last:border-0">
      <td className="whitespace-nowrap py-2 pr-4 text-content-secondary">{formatDateTime(entry.occurred_at)}</td>
      <td className="py-2 pr-4 text-content">{entry.event_type}</td>
      {/* Raw destination sink string, shown verbatim in mono per dashboard convention. */}
      <td className={MONO_CELL}>{entry.destination}</td>
      <td className="py-2 pr-4">
        <Badge color={dispositionBadgeColor(entry.disposition)} size="sm">
          {badgeLabelKey ? t(badgeLabelKey) : entry.disposition}
        </Badge>
      </td>
      <td className="py-2 pr-4 text-right text-content-secondary">
        {isCaptureBlocked ? (
          // byte 0 here is "no bytes captured/uploaded", NOT "0 bytes uploaded".
          <span title={t('privacy.egress.captureBlockedBytesHint')}>—</span>
        ) : (
          formatBytes(entry.byte_count)
        )}
      </td>
      <td className="py-2 pr-4 text-right text-content-secondary">
        {isCaptureBlocked ? '—' : formatNumber(entry.recipient_count)}
      </td>
      {/* Raw consent snapshot string, shown verbatim in mono. */}
      <td className={MONO_CELL}>{entry.consent_state}</td>
    </tr>
  )
}
