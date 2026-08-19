/**
 * #9466: today's AI token usage + estimated BYOK spend, on the privacy/egress
 * trust surface — "what left this device" and "what it cost" are two faces of
 * the same question for a bring-your-own-key product.
 *
 * Data source is the existing `get_token_usage` Tauri IPC (the #6266
 * reaper-durable daily ledger — today only, resets on restart; the copy says
 * so). Spend is priced from the LOCAL `ai-usage-pricing` table, never a
 * network lookup. Outside the Tauri runtime (plain browser dashboard) the
 * card renders nothing — the IPC surface does not exist there.
 */

import { useQuery } from '@tanstack/react-query'
import { Coins } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Badge, Card, CardTitle } from '../../components/ui'
import { iconSize } from '../../styles/tokens'
import { formatNumber } from '../../utils/formatters'
import { isTauriRuntime } from '../../utils/platform'
import { estimateSpendUsd, formatUsd } from './ai-usage-pricing'

export interface TokenUsagePayload {
  totalInputTokens: number
  totalOutputTokens: number
  dailyBudget: number
  budgetRemaining: number | null
  model: string | null
  provider: string | null
}

async function fetchTokenUsage(): Promise<TokenUsagePayload> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<TokenUsagePayload>('get_token_usage')
}

export default function AiUsageCard() {
  const { t } = useTranslation()
  const inTauri = isTauriRuntime()

  const { data } = useQuery({
    queryKey: ['ai-token-usage'],
    queryFn: fetchTokenUsage,
    enabled: inTauri,
    refetchInterval: 60_000,
    retry: false,
  })

  // Browser dashboard (no IPC) or fetch failure: this surface simply absent.
  if (!inTauri || !data) return null

  const total = data.totalInputTokens + data.totalOutputTokens
  const estimate = estimateSpendUsd(data.model, data.totalInputTokens, data.totalOutputTokens)
  const budgetUsedPct = data.dailyBudget > 0 ? Math.min(100, Math.round((total / data.dailyBudget) * 100)) : null

  return (
    <Card id="section-ai-usage" variant="default" padding="lg" className="mb-4">
      <div className="mb-1 flex items-center gap-2">
        <Coins aria-hidden className={`${iconSize.base} text-content-secondary`} />
        <CardTitle>{t('privacy.aiUsage.title')}</CardTitle>
      </div>
      <p className="mb-4 text-content-secondary text-sm">{t('privacy.aiUsage.description')}</p>

      <dl className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <div>
          <dt className="text-content-secondary text-xs">{t('privacy.aiUsage.inputTokens')}</dt>
          <dd className="text-sm tabular-nums">{formatNumber(data.totalInputTokens)}</dd>
        </div>
        <div>
          <dt className="text-content-secondary text-xs">{t('privacy.aiUsage.outputTokens')}</dt>
          <dd className="text-sm tabular-nums">{formatNumber(data.totalOutputTokens)}</dd>
        </div>
        <div>
          <dt className="text-content-secondary text-xs">{t('privacy.aiUsage.totalTokens')}</dt>
          <dd className="text-sm tabular-nums">{formatNumber(total)}</dd>
        </div>
        <div>
          <dt className="text-content-secondary text-xs">{t('privacy.aiUsage.estimatedSpend')}</dt>
          <dd className="text-sm tabular-nums">
            {estimate ? (
              <>
                {formatUsd(estimate.usd)}{' '}
                <span className="text-content-secondary text-xs">{t('privacy.aiUsage.estimateSuffix')}</span>
              </>
            ) : (
              <span className="text-content-secondary text-xs">{t('privacy.aiUsage.pricingUnknown')}</span>
            )}
          </dd>
        </div>
      </dl>

      <div className="mt-3 flex flex-wrap items-center gap-2 text-content-secondary text-xs">
        {data.model ? <Badge color="default">{data.model}</Badge> : null}
        {budgetUsedPct !== null ? (
          <span>
            {t('privacy.aiUsage.budgetUsed', {
              pct: budgetUsedPct,
              budget: formatNumber(data.dailyBudget),
            })}
          </span>
        ) : (
          <span>{t('privacy.aiUsage.noBudget')}</span>
        )}
        <span>{t('privacy.aiUsage.todayOnlyNote')}</span>
      </div>
    </Card>
  )
}
