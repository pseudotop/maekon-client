/**
 * #7600: audit-page affordance for the durable audit-log hash-chain
 * integrity check (ADR-072). Before this component existed, the real
 * verification (`verify_audit_chain()`) had a Tauri IPC command
 * (`verify_audit_log`) with zero webview callers and no HTTP route — an
 * advertised compliance capability with no delivery path in the shipped UI.
 * This "Verify integrity" button reaches it via the IS_TAURI switch: desktop
 * invoke() in the Tauri webview, HTTP `GET /audit/verify` in the standalone
 * browser surface — and renders the pass/fail + first-broken-entry result.
 */
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchAuditVerify } from '../../api/client'
import type { AuditChainReport } from '../../api/contracts'
import { Alert, Button, Spinner } from '../../components/ui'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { IS_TAURI } from '../../utils/platform'

async function invokeVerifyAuditLog(): Promise<AuditChainReport> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<AuditChainReport>('verify_audit_log')
}

export default function ChainVerifySection() {
  const { t } = useTranslation()
  const [report, setReport] = useState<AuditChainReport | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const handleVerify = async () => {
    setLoading(true)
    setError(null)
    try {
      // Desktop webview: real IPC command with zero prior callers (#7600).
      // Standalone browser: the new GET /audit/verify HTTP route (same
      // underlying SqliteStorage::verify_audit_chain via AuditChainVerifierPort).
      const result = IS_TAURI ? await invokeVerifyAuditLog() : await fetchAuditVerify()
      setReport(result)
    } catch (e) {
      setError(e instanceof Error ? e.message : t('auditLog.verifyFailed', 'Verification failed'))
      setReport(null)
    } finally {
      setLoading(false)
    }
  }

  return (
    <div id="section-chain-verify" className="space-y-3 rounded-lg border border-DEFAULT p-4">
      <div className="flex items-center justify-between gap-4">
        <div>
          <h2 className={cn(typography.label, colors.text.primary)}>
            {t('auditLog.chainIntegrity', 'Audit chain integrity')}
          </h2>
          <p className={cn(typography.caption, colors.text.secondary)}>
            {t(
              'auditLog.chainIntegrityHint',
              'Verifies the tamper-evident SHA-256 hash chain of the durable audit log.',
            )}
          </p>
        </div>
        <Button variant="secondary" size="sm" onClick={handleVerify} disabled={loading}>
          {loading ? <Spinner size="sm" /> : t('auditLog.verifyIntegrity', 'Verify integrity')}
        </Button>
      </div>

      {error && (
        <Alert variant="error" title={t('auditLog.verifyFailed', 'Verification failed')}>
          {error}
        </Alert>
      )}

      {report?.ok === true && (
        <Alert variant="success" title={t('auditLog.chainIntact', 'Chain intact')}>
          {t('auditLog.chainVerifiedCount', {
            count: report.verified_count,
            defaultValue: '{{count}} entries verified.',
          })}
          {report.legacy_unchained_count > 0 &&
            ` ${t('auditLog.chainLegacyCount', {
              count: report.legacy_unchained_count,
              defaultValue: '{{count}} legacy (pre-hash-chain) entries excluded.',
            })}`}
        </Alert>
      )}

      {report?.ok === false && (
        <Alert variant="error" title={t('auditLog.chainBroken', 'Integrity break detected')}>
          {report.first_break
            ? t('auditLog.chainBrokenDetail', {
                seq: report.first_break.seq,
                reason: report.first_break.reason,
                defaultValue: 'First broken entry at seq {{seq}}: {{reason}}',
              })
            : t('auditLog.chainBrokenUnknown', 'Chain verification failed for an unknown reason.')}
        </Alert>
      )}
    </div>
  )
}
