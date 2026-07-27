/**
 * Privacy export section — backup/restore functionality.
 */

import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Alert, Button, Card, CardTitle } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { typography } from '../../styles/tokens'
import { resolveApiUrl, withResolvedLocalAuthHeaders } from '../../utils/api-base'
import type { PrivacyContext } from './PrivacyLayout'

export default function ExportSection() {
  const { t } = useTranslation()
  const {
    backupOptions,
    setBackupOptions,
    backupMutation,
    restoreMutation,
    restoreResult,
    setRestoreResult,
    restoreError,
    setRestoreError,
    fileInputRef,
    handleBackup,
    handleRestoreFile,
  } = useTypedOutletContext<PrivacyContext>('Privacy')

  // #8056 P2-3: GDPR Art.20 full data-portability export. Streams the archive
  // from /api/export/full and triggers a browser download.
  const [fullExporting, setFullExporting] = useState(false)
  const [fullExportError, setFullExportError] = useState<string | null>(null)

  const handleFullExport = async () => {
    setFullExportError(null)
    setFullExporting(true)
    try {
      const url = await resolveApiUrl('/api/export/full')
      const response = await fetch(url, await withResolvedLocalAuthHeaders())
      if (!response.ok) {
        throw new Error(response.statusText || `HTTP ${response.status}`)
      }
      const blob = await response.blob()
      const objectUrl = URL.createObjectURL(blob)
      const anchor = document.createElement('a')
      anchor.href = objectUrl
      anchor.download = `maekon_data_export_${new Date().toISOString().slice(0, 10)}.json`
      document.body.appendChild(anchor)
      anchor.click()
      anchor.remove()
      URL.revokeObjectURL(objectUrl)
    } catch (error) {
      setFullExportError(error instanceof Error ? error.message : String(error))
    } finally {
      setFullExporting(false)
    }
  }

  return (
    <Card id="section-export" variant="default" padding="lg">
      <CardTitle className="mb-4">{t('backup.title')}</CardTitle>
      <p className="mb-4 text-content-secondary text-sm">{t('backup.description')}</p>

      {/* Restore error */}
      {restoreError && (
        <Alert variant="error" title={restoreError} className="mb-4">
          <button type="button" className="mt-2 text-sm underline" onClick={() => setRestoreError(null)}>
            {t('common.dismiss', 'Dismiss')}
          </button>
        </Alert>
      )}

      {/* Restore result */}
      {restoreResult && (
        <div
          className={`mb-4 rounded-lg p-4 ${
            restoreResult.success
              ? 'border border-status-connected bg-semantic-success/20 text-semantic-success'
              : 'border border-status-error bg-semantic-error/20 text-semantic-error'
          }`}
        >
          <div className={`${typography.weight.medium}`}>
            {restoreResult.success ? t('backup.restoreSuccess') : t('backup.restoreFailed')}
          </div>
          <div className="mt-2 space-y-1 text-sm">
            {restoreResult.restored.settings && <div>{t('backup.settingsRestored')}</div>}
            {restoreResult.restored.tags > 0 && (
              <div>{t('backup.tagsRestored', { count: restoreResult.restored.tags })}</div>
            )}
            {restoreResult.restored.frame_tags > 0 && (
              <div>{t('backup.frameTagsRestored', { count: restoreResult.restored.frame_tags })}</div>
            )}
            {restoreResult.restored.events > 0 && (
              <div>{t('backup.eventsRestored', { count: restoreResult.restored.events })}</div>
            )}
            {restoreResult.restored.frames > 0 && (
              <div>{t('backup.framesRestored', { count: restoreResult.restored.frames })}</div>
            )}
          </div>
          {restoreResult.errors.length > 0 && (
            <div className="mt-2 text-sm">
              <div className={`${typography.weight.medium}`}>{t('backup.errors')}:</div>
              <ul className="list-inside list-disc">
                {restoreResult.errors.slice(0, 5).map((err) => (
                  <li key={err}>{err}</li>
                ))}
                {restoreResult.errors.length > 5 && (
                  <li>...{t('backup.moreErrors', { count: restoreResult.errors.length - 5 })}</li>
                )}
              </ul>
            </div>
          )}
          <button
            type="button"
            onClick={() => setRestoreResult(null)}
            className="mt-3 text-sm underline hover:no-underline"
          >
            {t('common.close')}
          </button>
        </div>
      )}

      {/* Backup options */}
      <div className="mb-4">
        <span className={`mb-2 block ${typography.weight.medium} text-content-strong text-sm`}>
          {t('backup.includeData')}
        </span>
        <div className="flex flex-wrap gap-2">
          <Button
            variant={backupOptions.include_settings ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => setBackupOptions((prev) => ({ ...prev, include_settings: !prev.include_settings }))}
          >
            {t('backup.settings')}
          </Button>
          <Button
            variant={backupOptions.include_tags ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => setBackupOptions((prev) => ({ ...prev, include_tags: !prev.include_tags }))}
          >
            {t('backup.tags')}
          </Button>
          <Button
            variant={backupOptions.include_events ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => setBackupOptions((prev) => ({ ...prev, include_events: !prev.include_events }))}
          >
            {t('backup.events')}
          </Button>
          <Button
            variant={backupOptions.include_frames ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => setBackupOptions((prev) => ({ ...prev, include_frames: !prev.include_frames }))}
          >
            {t('backup.frames')}
          </Button>
        </div>
        <p className="mt-2 text-content-tertiary text-xs">{t('backup.optionsHint')}</p>
      </div>

      {/* Download + Restore buttons */}
      <div className="flex flex-wrap gap-3">
        <Button
          data-testid="download-backup"
          variant="primary"
          onClick={handleBackup}
          isLoading={backupMutation.isPending}
        >
          {backupMutation.isPending ? t('backup.creating') : t('backup.download')}
        </Button>

        <div>
          <input
            ref={fileInputRef}
            type="file"
            accept=".json"
            onChange={(e) => void handleRestoreFile(e)}
            className="hidden"
          />
          <Button
            variant="secondary"
            onClick={() => fileInputRef.current?.click()}
            isLoading={restoreMutation.isPending}
          >
            {restoreMutation.isPending ? t('backup.restoring') : t('backup.restore')}
          </Button>
        </div>
      </div>

      {/* Error display */}
      {backupMutation.isError && (
        <div className="mt-3 text-semantic-error text-sm">
          {t('backup.downloadFailed')}: {(backupMutation.error as Error).message}
        </div>
      )}
      {restoreMutation.isError && (
        <div className="mt-3 text-semantic-error text-sm">
          {t('backup.restoreFailed')}: {(restoreMutation.error as Error).message}
        </div>
      )}

      {/* #8056 P2-3: GDPR Art.20 full data-portability export. */}
      <div className="mt-6 border-border-subtle border-t pt-4">
        <span className={`mb-1 block ${typography.weight.medium} text-content-strong text-sm`}>
          {t('backup.fullExportTitle', { defaultValue: 'Export all my data (GDPR Art. 20)' })}
        </span>
        <p className="mb-3 text-content-tertiary text-xs">
          {t('backup.fullExportDescription', {
            defaultValue:
              'Download a machine-readable JSON archive of every personal-data table this device holds — activity segments and summaries, suggestions, coaching, habit streaks, the memory graph, frame annotations, AI chats, focus metrics, work sessions and digests. Free-text content is masked.',
          })}
        </p>
        <Button
          data-testid="download-full-export"
          variant="secondary"
          onClick={handleFullExport}
          isLoading={fullExporting}
        >
          {fullExporting
            ? t('backup.exporting', { defaultValue: 'Preparing export…' })
            : t('backup.fullExportButton', { defaultValue: 'Download full data export' })}
        </Button>
        {fullExportError && (
          <div className="mt-2 text-semantic-error text-sm">
            {t('backup.fullExportFailed', { defaultValue: 'Export failed' })}: {fullExportError}
          </div>
        )}
      </div>
    </Card>
  )
}
