import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { type AuditEntry, fetchAuditLogs } from '../../api/client'
import { Select } from '../../components/ui'
import { Badge } from '../../components/ui/Badge'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { typography } from '../../styles/tokens'

function statusBadge(s: string, t: (key: string) => string) {
  switch (s) {
    case 'Completed':
      return (
        <Badge color="success" size="sm">
          {t('automation.successful')}
        </Badge>
      )
    case 'Failed':
      return (
        <Badge color="error" size="sm">
          {t('automation.failed')}
        </Badge>
      )
    case 'Denied':
      return (
        <Badge color="warning" size="sm">
          {t('automation.denied')}
        </Badge>
      )
    case 'Timeout':
      return (
        <Badge color="purple" size="sm">
          {t('automation.timeout')}
        </Badge>
      )
    case 'Started':
      return (
        <Badge color="info" size="sm">
          {t('automation.started')}
        </Badge>
      )
    default:
      return (
        <Badge color="default" size="sm">
          {s}
        </Badge>
      )
  }
}

export default function EntriesSection() {
  const { t } = useTranslation()
  const [statusFilter, setStatusFilter] = useState<string>('')

  const { data: auditLogs } = useQuery({
    queryKey: ['auditLogPage', statusFilter],
    queryFn: () => fetchAuditLogs(100, statusFilter || undefined),
    refetchInterval: 10_000,
  })

  return (
    <Card id="section-entries">
      <CardHeader>
        <div className="flex items-center justify-between">
          <CardTitle>{t('auditLog.entries')}</CardTitle>
          <Select
            value={statusFilter}
            selectSize="sm"
            onChange={(e) => setStatusFilter(e.target.value)}
            className="w-auto min-w-[9rem]"
          >
            <option value="">{t('common.all')}</option>
            <option value="Completed">{t('automation.successful')}</option>
            <option value="Failed">{t('automation.failed')}</option>
            <option value="Denied">{t('automation.denied')}</option>
            <option value="Timeout">{t('automation.timeout')}</option>
          </Select>
        </div>
      </CardHeader>
      <CardContent>
        {(auditLogs?.length ?? 0) === 0 ? (
          <p className="py-4 text-center text-content-secondary text-sm">{t('common.noData')}</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-muted border-b">
                  <th className={`px-2 py-2 text-left ${typography.weight.medium} text-content-secondary`}>
                    {t('automation.time')}
                  </th>
                  <th className={`px-2 py-2 text-left ${typography.weight.medium} text-content-secondary`}>
                    {t('automation.actionType')}
                  </th>
                  <th className={`px-2 py-2 text-left ${typography.weight.medium} text-content-secondary`}>
                    {t('automation.statusLabel')}
                  </th>
                  <th className={`px-2 py-2 text-left ${typography.weight.medium} text-content-secondary`}>
                    {t('auditLog.details')}
                  </th>
                  <th className={`px-2 py-2 text-right ${typography.weight.medium} text-content-secondary`}>
                    {t('automation.elapsed')}
                  </th>
                </tr>
              </thead>
              <tbody>
                {(auditLogs ?? []).map((entry: AuditEntry) => (
                  <tr key={entry.entry_id} className="border-muted border-b">
                    <td className="whitespace-nowrap px-2 py-2 text-content-strong">
                      {new Date(entry.timestamp).toLocaleString()}
                    </td>
                    <td className="px-2 py-2 text-content-strong">{entry.action_type}</td>
                    <td className="px-2 py-2">{statusBadge(entry.status, t)}</td>
                    <td className="max-w-xs truncate px-2 py-2 text-content-secondary" title={entry.details ?? ''}>
                      {entry.details ?? '-'}
                    </td>
                    <td className="px-2 py-2 text-right text-content-strong">
                      {entry.elapsed_ms != null ? `${entry.elapsed_ms}ms` : '-'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </CardContent>
    </Card>
  )
}
