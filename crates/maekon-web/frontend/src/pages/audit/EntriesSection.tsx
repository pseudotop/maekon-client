import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { type AuditExportEntry, fetchAuditExport } from '../../api/client'
import { Select } from '../../components/ui'
import { Badge } from '../../components/ui/Badge'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { typography } from '../../styles/tokens'

/**
 * Render a stable, bounded correlation token without exposing raw command or
 * audit payload identifiers. FNV-1a is used only for display correlation, not
 * for security or integrity decisions.
 */
export function auditCorrelationLabel(entry: Pick<AuditExportEntry, 'command_id' | 'entry_id'>): string {
  const source = entry.command_id || entry.entry_id
  let hash = 0x811c9dc5
  for (let index = 0; index < source.length; index += 1) {
    hash ^= source.charCodeAt(index)
    hash = Math.imul(hash, 0x01000193)
  }
  return `audit-${(hash >>> 0).toString(16).padStart(8, '0')}`
}

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

  // #8114: the list reads the DURABLE audit_log table (`/audit/export`) —
  // the same integrity-verifiable source `/audit/verify` covers — instead of
  // the in-memory automation buffer. Consent grant/revoke and privacy
  // transitions (pause/resume, focus) only exist in the durable table, so the
  // buffer-backed list showed an incomplete picture after those events. The
  // automation buffer still feeds the runtime summary cards (a distinct,
  // labeled purpose in SummarySection).
  const { data: auditLogs } = useQuery({
    queryKey: ['auditDurablePage', statusFilter],
    queryFn: () => fetchAuditExport({ limit: 100, status: statusFilter || undefined }),
    refetchInterval: 10_000,
  })

  return (
    <Card id="section-entries">
      <CardHeader>
        <div className="flex items-center justify-between">
          <div>
            <CardTitle>{t('auditLog.entries')}</CardTitle>
            <p className="mt-1 text-content-tertiary text-xs">{t('auditLog.durableSourceNote')}</p>
          </div>
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
                    {t('automation.commandId')}
                  </th>
                  <th className={`px-2 py-2 text-right ${typography.weight.medium} text-content-secondary`}>
                    {t('automation.elapsed')}
                  </th>
                </tr>
              </thead>
              <tbody>
                {(auditLogs ?? []).map((entry: AuditExportEntry) => (
                  <tr key={entry.entry_id} className="border-muted border-b">
                    <td className="whitespace-nowrap px-2 py-2 text-content-strong">
                      {new Date(entry.timestamp).toLocaleString()}
                    </td>
                    <td className="px-2 py-2 text-content-strong">{entry.action_type}</td>
                    <td className="px-2 py-2">{statusBadge(entry.status, t)}</td>
                    <td className={`whitespace-nowrap px-2 py-2 ${typography.family.mono} text-content-secondary`}>
                      {auditCorrelationLabel(entry)}
                    </td>
                    <td className="px-2 py-2 text-right text-content-strong">
                      {entry.execution_time_ms != null ? `${entry.execution_time_ms}ms` : '-'}
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
