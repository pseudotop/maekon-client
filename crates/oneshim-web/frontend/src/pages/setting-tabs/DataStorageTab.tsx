import { useTranslation } from 'react-i18next'
import { Card, CardTitle, GuidancePanel, Input, Spinner } from '../../components/ui'
import { colors, form, iconSize, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { formatBytes, formatNumber } from '../../utils/formatters'
import { useLoadedFormData, useSettingsFormContext } from '../settings/SettingsFormContext'
import ToggleRow from './ToggleRow'

interface StorageCardProps {
  label: string
  value: string
  subValue: string
}

function StorageCard({ label, value, subValue }: StorageCardProps) {
  return (
    <Card variant="elevated" padding="md">
      <div className={cn('text-sm', colors.text.secondary)}>{label}</div>
      <div className={cn(`mt-1 ${typography.weight.bold} text-2xl`, colors.text.primary)}>{value}</div>
      <div className={cn('mt-1 text-xs', colors.text.tertiary)}>{subValue}</div>
    </Card>
  )
}

interface ExportButtonProps {
  label: string
  description: string
  onClick: () => void
  loading: boolean
}

function ExportButton({ label, description, onClick, loading }: ExportButtonProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={loading}
      className={`flex flex-col items-start rounded-lg border border-DEFAULT bg-surface-muted p-4 ${motion.colors} hover:border-brand-signal hover:bg-active disabled:cursor-not-allowed disabled:opacity-50`.trim()}
    >
      <div className="flex items-center gap-2">
        <svg
          className={cn(iconSize.md, colors.primary.text)}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          aria-hidden="true"
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth={2}
            d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4"
          />
        </svg>
        <span className={cn(`${typography.weight.medium}`, colors.text.primary)}>{label}</span>
        {loading && <Spinner size="sm" className={colors.primary.text} />}
      </div>
      <span className={cn('mt-1 text-xs', colors.text.tertiary)}>{description}</span>
    </button>
  )
}

export default function DataStorageTab() {
  const { t } = useTranslation()
  const { form: formCtx, data } = useSettingsFormContext()
  const formData = useLoadedFormData()
  const storageStats = data.storageStats
  const storageLoading = data.storageLoading
  const exportFormat = formCtx.exportFormat
  const exportLoading = formCtx.exportLoading

  return (
    <div className="space-y-6">
      <GuidancePanel
        title={t('settings.guidance.dataStorage.title')}
        description={t('settings.guidance.dataStorage.description')}
        items={[
          {
            title: t('settings.guidance.dataStorage.review.title'),
            description: t('settings.guidance.dataStorage.review.description'),
          },
          {
            title: t('settings.guidance.dataStorage.export.title'),
            description: t('settings.guidance.dataStorage.export.description'),
          },
          {
            title: t('settings.guidance.dataStorage.telemetry.title'),
            description: t('settings.guidance.dataStorage.telemetry.description'),
          },
        ]}
      />

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.storageStats')}</CardTitle>
        {storageLoading ? (
          <div className="flex h-32 items-center justify-center">
            <Spinner size="lg" className={colors.primary.text} />
          </div>
        ) : (
          <>
            {storageStats && (
              <div className="grid grid-cols-2 gap-4 md:grid-cols-4">
                <StorageCard
                  label={t('settings.totalSize')}
                  value={formatBytes(storageStats.total_size_bytes)}
                  subValue={`${t('settings.dbSize')}: ${formatBytes(storageStats.db_size_bytes)} / ${t('settings.frameSize')}: ${formatBytes(storageStats.frames_size_bytes)}`}
                />
                <StorageCard
                  label={t('settings.frameCount')}
                  value={formatNumber(storageStats.frame_count)}
                  subValue={t('settings.screenshots')}
                />
                <StorageCard
                  label={t('settings.eventCount')}
                  value={formatNumber(storageStats.event_count)}
                  subValue={t('settings.activityLogs')}
                />
                <StorageCard
                  label={t('settings.metricCount')}
                  value={formatNumber(storageStats.metric_count)}
                  subValue={t('settings.systemMeasure')}
                />
              </div>
            )}
            {storageStats?.oldest_data_date && storageStats?.newest_data_date && (
              <div className="mt-4 text-content-secondary text-sm">
                {t('settings.dataRange')}: {storageStats.oldest_data_date.split('T')[0]} ~{' '}
                {storageStats.newest_data_date.split('T')[0]}
              </div>
            )}
          </>
        )}
      </Card>

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.exportTitle')}</CardTitle>
        <p className="mb-4 text-content-secondary text-sm">{t('settings.exportDescription')}</p>

        <div className="mb-4 flex items-center gap-4">
          <span className="text-content-strong text-sm">{t('settings.exportFormatLabel')}:</span>
          <label className="flex cursor-pointer items-center">
            <input
              type="radio"
              name="exportFormat"
              value="json"
              checked={exportFormat === 'json'}
              onChange={() => formCtx.setExportFormat('json')}
              className={form.radio}
            />
            <span className="ml-2 text-content-strong">JSON</span>
          </label>
          <label className="flex cursor-pointer items-center">
            <input
              type="radio"
              name="exportFormat"
              value="csv"
              checked={exportFormat === 'csv'}
              onChange={() => formCtx.setExportFormat('csv')}
              className={form.radio}
            />
            <span className="ml-2 text-content-strong">CSV</span>
          </label>
        </div>

        <div className="grid grid-cols-1 gap-4 md:grid-cols-3">
          <ExportButton
            label={t('settings.exportMetricsLabel')}
            description={t('settings.exportMetricsDesc')}
            onClick={() => formCtx.handleExport('metrics')}
            loading={exportLoading === 'metrics'}
          />
          <ExportButton
            label={t('settings.exportEventsLabel')}
            description={t('settings.exportEventsDesc')}
            onClick={() => formCtx.handleExport('events')}
            loading={exportLoading === 'events'}
          />
          <ExportButton
            label={t('settings.exportFramesLabel')}
            description={t('settings.exportFramesDesc')}
            onClick={() => formCtx.handleExport('frames')}
            loading={exportLoading === 'frames'}
          />
        </div>
      </Card>

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.retentionTitle')}</CardTitle>
        <div className="grid grid-cols-1 gap-6 md:grid-cols-2">
          <div>
            <label htmlFor="settings-retention-days" className={form.label}>
              {t('settings.retentionDays')}
            </label>
            <Input
              id="settings-retention-days"
              type="number"
              min={1}
              max={365}
              value={formData.retention_days}
              onChange={(e) => formCtx.handleRootChange('retention_days', parseInt(e.target.value, 10) || 30)}
            />
            <p className={form.helper}>{t('settings.retentionAutoDelete')}</p>
          </div>
          <div>
            <label htmlFor="settings-max-storage-mb" className={form.label}>
              {t('settings.maxStorageMb')}
            </label>
            <Input
              id="settings-max-storage-mb"
              type="number"
              min={100}
              max={10000}
              step={100}
              value={formData.max_storage_mb}
              onChange={(e) => formCtx.handleRootChange('max_storage_mb', parseInt(e.target.value, 10) || 500)}
            />
            <p className={form.helper}>{t('settings.maxStorageOverflow')}</p>
          </div>
        </div>
      </Card>

      <Card id="section-telemetry" variant="default" padding="lg">
        <CardTitle sticky>{t('settings.telemetryTitle')}</CardTitle>
        <p className="mb-4 text-content-secondary text-sm">{t('settings.telemetryDesc')}</p>
        <div className="space-y-4">
          <ToggleRow
            label={t('settings.telemetryEnabled')}
            description={t('settings.telemetryEnabledDesc')}
            checked={formData.telemetry.enabled}
            onChange={(value) => formCtx.handleTelemetryChange('enabled', value)}
          />

          <div
            className={`space-y-4 border-DEFAULT border-l-2 pl-4 ${!formData.telemetry.enabled ? 'pointer-events-none opacity-50' : ''}`}
          >
            <ToggleRow
              label={t('settings.crashReports')}
              description={t('settings.crashReportsDesc')}
              checked={formData.telemetry.crash_reports}
              onChange={(value) => formCtx.handleTelemetryChange('crash_reports', value)}
            />
            <ToggleRow
              label={t('settings.usageStats')}
              description={t('settings.usageStatsDesc')}
              checked={formData.telemetry.usage_analytics}
              onChange={(value) => formCtx.handleTelemetryChange('usage_analytics', value)}
            />
            <ToggleRow
              label={t('settings.perfMetrics')}
              description={t('settings.perfMetricsDesc')}
              checked={formData.telemetry.performance_metrics}
              onChange={(value) => formCtx.handleTelemetryChange('performance_metrics', value)}
            />
          </div>
        </div>
      </Card>
    </div>
  )
}
