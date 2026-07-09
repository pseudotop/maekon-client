import { ClipboardList } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { EmptyState } from '../../components/ui'
import { Card, CardContent } from '../../components/ui/Card'
import { useTypedOutletContext } from '../../routes'
import { typography } from '../../styles/tokens'
import type { AuditOutletContext } from './AuditLayout'
import ChainVerifySection from './ChainVerifySection'

export default function SummarySection() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { auditLogs, stats } = useTypedOutletContext<AuditOutletContext>('Audit')

  // #7600: the hash-chain integrity affordance is independent of the
  // automation audit trail rendered below (a separate durable SQLite table,
  // ADR-072) — render it unconditionally, including on the empty-state path,
  // so the compliance capability stays reachable even with zero automation
  // executions logged.
  const chainVerify = <ChainVerifySection />

  // Empty state lives here (rather than in AuditLayout) so that the layout can
  // keep rendering <Outlet> unconditionally — see AuditLayout comment for why.
  if ((auditLogs?.length ?? 0) === 0 && (stats?.total_executions ?? 0) === 0) {
    return (
      <div className="space-y-6">
        {chainVerify}
        <EmptyState
          icon={<ClipboardList className="h-8 w-8" />}
          title={t('emptyState.auditLog.title')}
          description={t('emptyState.auditLog.description')}
          action={{ label: t('emptyState.auditLog.action'), onClick: () => navigate('/automation') }}
        />
      </div>
    )
  }

  return (
    <div className="space-y-6">
      {chainVerify}
      <div id="section-summary" className="grid grid-cols-2 gap-4 md:grid-cols-5">
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.totalExecutions')}</div>
            <div className={`mt-1 ${typography.stat.large} text-content`}>{stats?.total_executions ?? 0}</div>
          </CardContent>
        </Card>
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.successful')}</div>
            <div className={`mt-1 ${typography.stat.large} text-semantic-success`}>{stats?.successful ?? 0}</div>
          </CardContent>
        </Card>
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.failed')}</div>
            <div className={`mt-1 ${typography.stat.large} text-semantic-error`}>{stats?.failed ?? 0}</div>
          </CardContent>
        </Card>
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.denied')}</div>
            <div className={`mt-1 ${typography.stat.large} text-semantic-warning`}>{stats?.denied ?? 0}</div>
          </CardContent>
        </Card>
        <Card>
          <CardContent>
            <div className="text-content-secondary text-sm">{t('automation.successRate')}</div>
            <div className={`mt-1 ${typography.stat.large} text-semantic-success`}>
              {((stats?.success_rate ?? 0) * 100).toFixed(1)}%
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
