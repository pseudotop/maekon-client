import { CheckCircle2, CircleAlert } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Link, useInRouterContext } from 'react-router-dom'
import type { AiCapabilityId, FeatureCapabilitySnapshot } from '../api/contracts'
import {
  aiCapabilityCopyKey,
  aiCapabilityReadiness,
  aiReadinessActionCopyKey,
  aiReadinessActionRoute,
  aiReadinessReasonCopyKey,
} from '../features/aiReadiness'
import { iconSize, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { Alert } from './ui'

interface AiReadinessNoticeProps {
  snapshot: FeatureCapabilitySnapshot | null | undefined
  capabilityIds: readonly AiCapabilityId[]
  showReady?: boolean
  compact?: boolean
  className?: string
  onAction?: (route: string) => void
}

/**
 * Shared renderer for every AI product surface. It renders the backend-owned
 * reason/action pair verbatim and only translates the stable copy keys.
 */
export function AiReadinessNotice({
  snapshot,
  capabilityIds,
  showReady = false,
  compact = false,
  className,
  onAction,
}: AiReadinessNoticeProps) {
  const { t } = useTranslation()
  const inRouterContext = useInRouterContext()

  const entries = capabilityIds.map((capabilityId) => aiCapabilityReadiness(snapshot, capabilityId))
  const blocked = entries.filter((entry) => entry.status === 'blocked')
  const visible = showReady ? entries : blocked
  if (visible.length === 0) return null

  const allReady = blocked.length === 0
  return (
    <Alert
      variant={allReady ? 'success' : 'warning'}
      title={t(allReady ? 'aiReadiness.readyTitle' : 'aiReadiness.blockedTitle')}
      icon={allReady ? <CheckCircle2 className={iconSize.base} /> : <CircleAlert className={iconSize.base} />}
      className={cn(compact && 'p-2', className)}
      data-testid="ai-readiness-notice"
    >
      <ul className={cn('space-y-2', compact && 'space-y-1')}>
        {visible.map((entry) => {
          const route = aiReadinessActionRoute(entry)
          const actionCopy = t(aiReadinessActionCopyKey(entry))
          return (
            <li
              key={entry.capability_id}
              data-capability-id={entry.capability_id}
              data-readiness-status={entry.status}
              data-reason-code={entry.reason_code}
              className={cn('flex flex-wrap items-baseline gap-x-2 gap-y-1', compact && 'text-xs')}
            >
              <span className={typography.weight.medium}>{t(aiCapabilityCopyKey(entry.capability_id))}</span>
              <span>{t(aiReadinessReasonCopyKey(entry), entry.reason_code)}</span>
              <code className="text-[10px] text-content-tertiary">{entry.reason_code}</code>
              {entry.status === 'blocked' && route && onAction && (
                <button
                  type="button"
                  onClick={() => onAction(route)}
                  className="border-0 bg-transparent p-0 text-accent underline underline-offset-2"
                >
                  {actionCopy}
                </button>
              )}
              {entry.status === 'blocked' && route && !onAction && inRouterContext && (
                <Link to={route} className="text-accent underline underline-offset-2">
                  {actionCopy}
                </Link>
              )}
              {entry.status === 'blocked' && route && !onAction && !inRouterContext && (
                <a href={route} className="text-accent underline underline-offset-2">
                  {actionCopy}
                </a>
              )}
              {entry.status === 'blocked' && entry.action !== 'none' && !route && <span>{actionCopy}</span>}
            </li>
          )
        })}
      </ul>
    </Alert>
  )
}
