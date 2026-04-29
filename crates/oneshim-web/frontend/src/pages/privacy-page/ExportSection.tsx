/**
 * Privacy export section — backup/restore functionality.
 */

import { useTranslation } from 'react-i18next'
import { Alert, Button, Card, CardTitle } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { typography } from '../../styles/tokens'
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
    </Card>
  )
}
