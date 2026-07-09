/**
 * Memory-graph claims browser — read + user retraction (T1.3, #7911).
 *
 * Surfaces the durable ADR-023 "claims" the agent silently accumulates about the
 * user (a local second-brain belief store) via `GET /api/memory/claims`, with
 * kind / status / date-range filters. Each row exposes a one-click Retract
 * action (`POST /api/memory/claims/{id}/retract`) behind a destructive confirm:
 * retraction flips the claim to `retracted` — hiding it from the digest appendix
 * and retrieval — while PRESERVING the record and its provenance (never a
 * delete). Retracted claims are hidden by default; the status filter can surface
 * them for transparency. The evidence summary shows how many captured segments
 * support a belief (ids only, never content).
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Brain } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { type ClaimListParams, fetchClaims, retractClaim } from '../../api/client'
import type { Claim } from '../../api/contracts'
import {
  Alert,
  Badge,
  type BadgeProps,
  Button,
  Card,
  CardTitle,
  EmptyState,
  Select,
  Spinner,
} from '../../components/ui'
import { addToast } from '../../hooks/useToast'
import { typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { formatDateTime, formatPercent } from '../../utils/formatters'
import { ConfirmModal } from './PrivacyLayout'

type RangePreset = '24h' | '7d' | '30d' | 'all'
type KindFilter = 'all' | 'semantic' | 'episodic' | 'procedural' | 'reflective'
// `active_superseded` is the default: it sends NO status param, so the server
// applies its non-retracted default. The rest map to the server `status` value.
type StatusFilter = 'active_superseded' | 'active' | 'superseded' | 'retracted' | 'all'

const RANGE_PRESETS: RangePreset[] = ['24h', '7d', '30d', 'all']
const KIND_FILTERS: KindFilter[] = ['all', 'semantic', 'episodic', 'procedural', 'reflective']
const STATUS_FILTERS: StatusFilter[] = ['active_superseded', 'active', 'superseded', 'retracted', 'all']

const HEADER_CELL = cn('py-2 pr-4', typography.weight.medium)
const HEADER_CELL_RIGHT = cn('py-2 pr-4 text-right', typography.weight.medium)
const MONO_CELL = cn('py-2 pr-4 text-content-secondary text-xs', typography.family.mono)

/** Milliseconds per preset window; `all` has no window. */
const RANGE_WINDOW_MS: Record<Exclude<RangePreset, 'all'>, number> = {
  '24h': 24 * 60 * 60 * 1000,
  '7d': 7 * 24 * 60 * 60 * 1000,
  '30d': 30 * 24 * 60 * 60 * 1000,
}

/** Badge color per lifecycle status. */
function statusBadgeColor(status: string): BadgeProps['color'] {
  switch (status) {
    case 'active':
      return 'success'
    case 'superseded':
      return 'warning'
    case 'retracted':
      return 'error'
    default:
      return 'default'
  }
}

export default function ClaimsSection() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [range, setRange] = useState<RangePreset>('30d')
  const [kind, setKind] = useState<KindFilter>('all')
  const [status, setStatus] = useState<StatusFilter>('active_superseded')
  const [pendingRetract, setPendingRetract] = useState<Claim | null>(null)

  // Derive the server query params from the filters. `created_at` bounds are
  // epoch SECONDS; the default status filter sends no `status` param.
  const params = useMemo<ClaimListParams>(() => {
    const p: ClaimListParams = {}
    if (kind !== 'all') p.kind = kind
    if (status !== 'active_superseded') p.status = status
    if (range !== 'all') {
      const now = Date.now()
      p.from = Math.floor((now - RANGE_WINDOW_MS[range]) / 1000)
      p.to = Math.floor(now / 1000)
    }
    return p
  }, [kind, status, range])

  const { data, isLoading, isError, error } = useQuery({
    queryKey: ['claims', params],
    queryFn: () => fetchClaims(params),
  })

  const retractMutation = useMutation({
    mutationFn: (claimId: string) => retractClaim(claimId),
    onSuccess: (result) => {
      addToast(
        'success',
        result.already_retracted ? t('privacy.claims.retract.alreadyRetracted') : t('privacy.claims.retract.success'),
      )
      queryClient.invalidateQueries({ queryKey: ['claims'] })
      setPendingRetract(null)
    },
    onError: (err: Error) => {
      addToast('error', err.message)
      setPendingRetract(null)
    },
  })

  const claims = data?.claims ?? []
  const total = data?.total ?? 0
  const filtersActive = kind !== 'all' || status !== 'active_superseded' || range !== 'all'

  return (
    <Card id="section-claims" variant="default" padding="lg">
      <CardTitle className="mb-1">{t('privacy.claims.title')}</CardTitle>
      <p className="mb-4 text-content-secondary text-sm">{t('privacy.claims.description')}</p>

      {/* Filters */}
      <div className="mb-4 flex flex-wrap gap-4">
        <label className="flex flex-col gap-1 text-content-secondary text-xs">
          {t('privacy.claims.kindLabel')}
          <Select
            aria-label={t('privacy.claims.kindLabel')}
            value={kind}
            selectSize="sm"
            className="w-44"
            onChange={(e) => setKind(e.target.value as KindFilter)}
          >
            {KIND_FILTERS.map((value) => (
              <option key={value} value={value}>
                {t(`privacy.claims.kind.${value}`)}
              </option>
            ))}
          </Select>
        </label>
        <label className="flex flex-col gap-1 text-content-secondary text-xs">
          {t('privacy.claims.statusLabel')}
          <Select
            aria-label={t('privacy.claims.statusLabel')}
            value={status}
            selectSize="sm"
            className="w-48"
            onChange={(e) => setStatus(e.target.value as StatusFilter)}
          >
            {STATUS_FILTERS.map((value) => (
              <option key={value} value={value}>
                {t(`privacy.claims.status.${value}`)}
              </option>
            ))}
          </Select>
        </label>
        <label className="flex flex-col gap-1 text-content-secondary text-xs">
          {t('privacy.claims.rangeLabel')}
          <Select
            aria-label={t('privacy.claims.rangeLabel')}
            value={range}
            selectSize="sm"
            className="w-40"
            onChange={(e) => setRange(e.target.value as RangePreset)}
          >
            {RANGE_PRESETS.map((preset) => (
              <option key={preset} value={preset}>
                {t(`privacy.claims.range.${preset}`)}
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
        <Alert variant="error" title={t('privacy.claims.loadError')} className="mb-2">
          {(error as Error).message}
        </Alert>
      )}

      {!isLoading && !isError && claims.length === 0 && (
        <EmptyState
          icon={<Brain size={28} aria-hidden="true" />}
          title={filtersActive ? t('privacy.claims.empty.filteredTitle') : t('privacy.claims.empty.noneTitle')}
          description={filtersActive ? t('privacy.claims.empty.filteredDesc') : t('privacy.claims.empty.noneDesc')}
        />
      )}

      {!isLoading && !isError && claims.length > 0 && (
        <>
          <p className="mb-2 text-content-tertiary text-xs">
            {t('privacy.claims.count', { shown: claims.length, total })}
          </p>
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead>
                <tr className="border-muted border-b text-content-secondary text-xs">
                  <th className={HEADER_CELL}>{t('privacy.claims.col.time')}</th>
                  <th className={HEADER_CELL}>{t('privacy.claims.col.kind')}</th>
                  <th className={HEADER_CELL}>{t('privacy.claims.col.statement')}</th>
                  <th className={HEADER_CELL}>{t('privacy.claims.col.source')}</th>
                  <th className={HEADER_CELL_RIGHT}>{t('privacy.claims.col.confidence')}</th>
                  <th className={HEADER_CELL_RIGHT}>{t('privacy.claims.col.evidence')}</th>
                  <th className={HEADER_CELL}>{t('privacy.claims.col.status')}</th>
                  <th className={HEADER_CELL_RIGHT}>{t('privacy.claims.col.actions')}</th>
                </tr>
              </thead>
              <tbody>
                {claims.map((claim) => (
                  <ClaimRow key={claim.claim_id} claim={claim} onRetract={() => setPendingRetract(claim)} />
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      <ConfirmModal
        isOpen={pendingRetract !== null}
        title={t('privacy.claims.retract.confirmTitle')}
        message={t('privacy.claims.retract.confirmMessage', { text: pendingRetract?.text ?? '' })}
        note={t('privacy.claims.retract.confirmNote')}
        confirmText={t('privacy.claims.retract.confirmButton')}
        isDangerous
        onConfirm={() => {
          if (pendingRetract) retractMutation.mutate(pendingRetract.claim_id)
        }}
        onCancel={() => setPendingRetract(null)}
      />
    </Card>
  )
}

function ClaimRow({ claim, onRetract }: { claim: Claim; onRetract: () => void }) {
  const { t } = useTranslation()
  const kindLabelKey = KIND_FILTERS.includes(claim.kind as KindFilter) ? `privacy.claims.kind.${claim.kind}` : null
  const isRetracted = claim.status === 'retracted'

  return (
    <tr className="border-muted/50 border-b align-top last:border-0">
      <td className="whitespace-nowrap py-2 pr-4 text-content-secondary">
        {formatDateTime(new Date(claim.created_at * 1000).toISOString())}
      </td>
      <td className="py-2 pr-4 text-content">{kindLabelKey ? t(kindLabelKey) : claim.kind}</td>
      <td className="py-2 pr-4 text-content">{claim.text}</td>
      {/* Raw provenance source string, shown verbatim in mono. */}
      <td className={MONO_CELL}>{claim.source}</td>
      <td className="py-2 pr-4 text-right text-content-secondary">{formatPercent(claim.confidence * 100, 0)}</td>
      <td className="py-2 pr-4 text-right text-content-secondary">
        {claim.evidence_count > 0 ? (
          <span title={claim.evidence_segment_ids.join(', ')}>{claim.evidence_count}</span>
        ) : (
          '—'
        )}
      </td>
      <td className="py-2 pr-4">
        <Badge color={statusBadgeColor(claim.status)} size="sm">
          {t(`privacy.claims.status.${claim.status}`)}
        </Badge>
      </td>
      <td className="py-2 pr-4 text-right">
        {isRetracted ? (
          <span className="text-content-tertiary text-xs">{t('privacy.claims.retractedLabel')}</span>
        ) : (
          <Button variant="danger" size="sm" onClick={onRetract}>
            {t('privacy.claims.retract.button')}
          </Button>
        )}
      </td>
    </tr>
  )
}
