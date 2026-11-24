import { CheckCircle2, CircleAlert } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Link, useInRouterContext } from 'react-router-dom'
import type { AiCapabilityId, AiCapabilityReadiness, FeatureCapabilitySnapshot } from '../api/contracts'
import {
  aiCapabilityCopyKey,
  aiCapabilityReadiness,
  aiReadinessActionCopyKey,
  aiReadinessActionRoute,
  aiReadinessReasonCopyKey,
} from '../features/aiReadiness'
import { iconSize, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { SupportDetails } from './SupportDetails'
import { Alert } from './ui'

interface AiReadinessNoticeProps {
  snapshot: FeatureCapabilitySnapshot | null | undefined
  capabilityIds: readonly AiCapabilityId[]
  showReady?: boolean
  compact?: boolean
  className?: string
  onAction?: (route: string) => void
}

/** Capabilities that share one backend cause, and therefore one next action. */
interface ReadinessGroup {
  readonly key: string
  readonly lead: AiCapabilityReadiness
  readonly entries: readonly AiCapabilityReadiness[]
}

/**
 * #13737: Settings > AI & Automation used to stack one warning per blocked
 * capability — up to seven lines of the same sentence when the cause was
 * shared. The backend derives the action from the reason alone, so grouping by
 * the full (status, reason, action) triple never merges two different next
 * steps; it only folds repeats. Order follows the first capability of each
 * cause, so the caller's `capabilityIds` order still decides the reading order.
 */
function groupByCause(entries: readonly AiCapabilityReadiness[]): ReadinessGroup[] {
  const groups = new Map<string, { lead: AiCapabilityReadiness; entries: AiCapabilityReadiness[] }>()
  for (const entry of entries) {
    const key = [entry.status, entry.reason_code, entry.action, entry.action_copy_key].join('|')
    const group = groups.get(key)
    if (group) group.entries.push(entry)
    else groups.set(key, { lead: entry, entries: [entry] })
  }
  return [...groups].map(([key, group]) => ({ key, ...group }))
}

type ListFormatConstructor = new (
  locale: string | undefined,
  options: { style: 'long'; type: 'conjunction' },
) => { format(items: readonly string[]): string }

/**
 * "A, B, and C", joined with the active language's own conjunction. `Intl.ListFormat` is
 * ES2021 and the app's TypeScript lib stops at ES2020, so it is looked up
 * rather than referenced; every supported webview ships it.
 */
function formatList(items: readonly string[], language: string | undefined): string {
  const ListFormat = (Intl as unknown as { ListFormat?: ListFormatConstructor }).ListFormat
  if (!ListFormat) return items.join(', ')
  try {
    return new ListFormat(language, { style: 'long', type: 'conjunction' }).format(items)
  } catch {
    return items.join(', ')
  }
}

/**
 * Shared renderer for every AI product surface. It renders the backend-owned
 * reason/action pair and only translates the stable copy keys.
 *
 * The reason code itself is an internal identifier (`compiled_capability_missing`),
 * so it stays out of the default view: it is kept on `data-reason-code` for
 * tests and in the collapsed support details for whoever helps the user (#13737).
 */
export function AiReadinessNotice({
  snapshot,
  capabilityIds,
  showReady = false,
  compact = false,
  className,
  onAction,
}: AiReadinessNoticeProps) {
  const { t, i18n } = useTranslation()
  const inRouterContext = useInRouterContext()

  const entries = capabilityIds.map((capabilityId) => aiCapabilityReadiness(snapshot, capabilityId))
  const blocked = entries.filter((entry) => entry.status === 'blocked')
  const visible = showReady ? entries : blocked
  if (visible.length === 0) return null

  const allReady = blocked.length === 0
  const groups = groupByCause(visible)
  const supportText = visible.map((entry) => `${entry.capability_id}: ${entry.reason_code}`).join('\n')

  return (
    <Alert
      variant={allReady ? 'success' : 'warning'}
      title={t(allReady ? 'aiReadiness.readyTitle' : 'aiReadiness.blockedTitle')}
      icon={allReady ? <CheckCircle2 className={iconSize.base} /> : <CircleAlert className={iconSize.base} />}
      className={cn(compact && 'p-2', className)}
      data-testid="ai-readiness-notice"
    >
      <ul className={cn('space-y-2', compact && 'space-y-1')}>
        {groups.map(({ key, lead, entries: members }) => {
          const route = aiReadinessActionRoute(lead)
          const actionCopy = t(aiReadinessActionCopyKey(lead))
          const names = members.map((entry) => t(aiCapabilityCopyKey(entry.capability_id)))
          return (
            <li
              key={key}
              data-capability-ids={members.map((entry) => entry.capability_id).join(' ')}
              data-readiness-status={lead.status}
              data-reason-code={lead.reason_code}
              className={cn('flex flex-wrap items-baseline gap-x-2 gap-y-1', compact && 'text-xs')}
            >
              <span className={typography.weight.medium}>{formatList(names, i18n?.language)}</span>
              <span>{t(aiReadinessReasonCopyKey(lead), t('aiReadiness.unknownReason'))}</span>
              {lead.status === 'blocked' && route && onAction && (
                <button
                  type="button"
                  onClick={() => onAction(route)}
                  className="border-0 bg-transparent p-0 text-accent underline underline-offset-2"
                >
                  {actionCopy}
                </button>
              )}
              {lead.status === 'blocked' && route && !onAction && inRouterContext && (
                <Link to={route} className="text-accent underline underline-offset-2">
                  {actionCopy}
                </Link>
              )}
              {lead.status === 'blocked' && route && !onAction && !inRouterContext && (
                <a href={route} className="text-accent underline underline-offset-2">
                  {actionCopy}
                </a>
              )}
              {lead.status === 'blocked' && lead.action !== 'none' && !route && <span>{actionCopy}</span>}
            </li>
          )
        })}
      </ul>
      <SupportDetails detail={supportText} />
    </Alert>
  )
}
