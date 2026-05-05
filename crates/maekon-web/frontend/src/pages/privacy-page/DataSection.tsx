/**
 * Privacy data section — storage stats cards + delete by date range.
 */

import { BarChart3, Calendar, Camera, FileText, HardDrive } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Button, Card, CardTitle, Input } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { iconSize, typography } from '../../styles/tokens'
import { formatBytes, formatNumber } from '../../utils/formatters'
import type { PrivacyContext } from './PrivacyLayout'
import { DataCard } from './PrivacyLayout'

type DataType = 'events' | 'frames' | 'metrics' | 'processes' | 'idle'

export default function DataSection() {
  const { t } = useTranslation()
  const {
    storageStats,
    fromDate,
    setFromDate,
    toDate,
    setToDate,
    selectedDataTypes,
    handleDataTypeToggle,
    deleteRangeMutation,
    setShowDeleteRangeModal,
    DATA_TYPE_LABELS,
    getDateRangeText,
  } = useTypedOutletContext<PrivacyContext>('Privacy')

  return (
    <>
      {/* Storage stats */}
      <Card id="section-data" variant="default" padding="lg">
        <CardTitle className="mb-4">{t('privacy.currentData')}</CardTitle>
        {storageStats && (
          <>
            <div className="grid grid-cols-2 gap-4 md:grid-cols-5">
              <DataCard
                label={t('privacy.eventsLabel')}
                value={formatNumber(storageStats.event_count)}
                icon={<FileText className={`${iconSize.base}`} />}
              />
              <DataCard
                label={t('privacy.screenshotsLabel')}
                value={formatNumber(storageStats.frame_count)}
                icon={<Camera className={`${iconSize.base}`} />}
              />
              <DataCard
                label={t('privacy.metricsLabel')}
                value={formatNumber(storageStats.metric_count)}
                icon={<BarChart3 className={`${iconSize.base}`} />}
              />
              <DataCard
                label={t('privacy.storageSizeLabel')}
                value={formatBytes(storageStats.total_size_bytes)}
                icon={<HardDrive className={`${iconSize.base}`} />}
              />
              <DataCard
                label={t('privacy.dataRangeLabel')}
                value={getDateRangeText()}
                icon={<Calendar className={`${iconSize.base}`} />}
                small
              />
            </div>
            <div className="mt-4 text-content-tertiary text-sm">
              {t('settings.dbSize')}: {formatBytes(storageStats.db_size_bytes)} / {t('settings.frameSize')}:{' '}
              {formatBytes(storageStats.frames_size_bytes)}
            </div>
          </>
        )}
      </Card>

      {/* Delete by range */}
      <Card variant="default" padding="lg">
        <CardTitle className="mb-4">{t('privacy.deleteByRangeTitle')}</CardTitle>
        <p className="mb-4 text-content-secondary text-sm">{t('privacy.deleteByRangeDesc')}</p>

        <div className="mb-4 grid grid-cols-1 gap-4 md:grid-cols-2">
          <div>
            <label
              htmlFor="privacy-start-date"
              className={`mb-2 block ${typography.weight.medium} text-content-strong text-sm`}
            >
              {t('privacy.startDate')}
            </label>
            <Input id="privacy-start-date" type="date" value={fromDate} onChange={(e) => setFromDate(e.target.value)} />
          </div>
          <div>
            <label
              htmlFor="privacy-end-date"
              className={`mb-2 block ${typography.weight.medium} text-content-strong text-sm`}
            >
              {t('privacy.endDate')}
            </label>
            <Input id="privacy-end-date" type="date" value={toDate} onChange={(e) => setToDate(e.target.value)} />
          </div>
        </div>

        <div className="mb-4">
          <span className={`mb-2 block ${typography.weight.medium} text-content-strong text-sm`}>
            {t('privacy.dataTypesHint')}
          </span>
          <div className="flex flex-wrap gap-2">
            {(Object.entries(DATA_TYPE_LABELS) as [DataType, string][]).map(([type, label]) => (
              <Button
                key={type}
                variant={selectedDataTypes.includes(type) ? 'primary' : 'secondary'}
                size="sm"
                onClick={() => handleDataTypeToggle(type)}
              >
                {label}
              </Button>
            ))}
          </div>
        </div>

        <Button
          data-testid="delete-range"
          variant="warning"
          onClick={() => setShowDeleteRangeModal(true)}
          disabled={!fromDate || !toDate}
          isLoading={deleteRangeMutation.isPending}
        >
          {deleteRangeMutation.isPending ? t('privacy.deleting') : t('privacy.deleteRangeButton')}
        </Button>
      </Card>
    </>
  )
}
