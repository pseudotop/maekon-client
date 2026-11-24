/**
 * Realtime stream status dot, shared by the Dashboard header and the System
 * Metrics page (#13735 moved the realtime card there).
 */

import { useTranslation } from 'react-i18next'
import type { ConnectionStatus } from '../../hooks/useSSE'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

const statusConfig = {
  connecting: { color: colors.status.connecting, textKey: 'dashboard.connecting' },
  connected: { color: colors.status.connected, textKey: 'dashboard.connected' },
  disconnected: { color: colors.status.disconnected, textKey: 'dashboard.disconnected' },
  error: { color: colors.status.error, textKey: 'dashboard.error' },
} as const

export default function ConnectionIndicator({ status }: { status: ConnectionStatus }) {
  const { t } = useTranslation()
  const config = statusConfig[status]

  return (
    <div className={cn('flex items-center space-x-2', typography.body)}>
      <span className={cn('h-2 w-2 rounded-full', config.color, status === 'connected' && 'animate-pulse')} />
      <span className={colors.text.secondary}>{t(config.textKey)}</span>
    </div>
  )
}
