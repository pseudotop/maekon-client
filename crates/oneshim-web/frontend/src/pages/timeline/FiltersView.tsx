/**
 * Timeline filters view — detailed filter management panel
 * for app filter, importance filter, and tag filter.
 */

import { useTranslation } from 'react-i18next'
import { Button, Card, CardTitle, Select } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { typography } from '../../styles/tokens'
import type { ImportanceFilter, TimelineContext } from './TimelineLayout'

export default function FiltersView() {
  const { t } = useTranslation()
  const {
    appFilter,
    setAppFilter,
    importanceFilter,
    setImportanceFilter,
    tagFilter,
    setTagFilter,
    appList,
    allTags,
    filteredFrames,
    frames,
  } = useTypedOutletContext<TimelineContext>('Timeline')

  const hasActiveFilters = appFilter !== 'all' || importanceFilter !== 'all' || tagFilter !== 'all'

  const clearAllFilters = () => {
    setAppFilter('all')
    setImportanceFilter('all')
    setTagFilter('all')
  }

  return (
    <Card variant="default" padding="lg">
      <div className="mb-4 flex items-center justify-between">
        <CardTitle>{t('timeline.filters', 'Filters')}</CardTitle>
        {hasActiveFilters && (
          <Button variant="secondary" size="sm" onClick={clearAllFilters}>
            {t('timeline.clearFilters', 'Clear All')}
          </Button>
        )}
      </div>

      <div className="space-y-6">
        {/* App filter */}
        <div>
          <label htmlFor="filters-app" className={`mb-2 block ${typography.weight.medium} text-content-strong text-sm`}>
            {t('timeline.app')}
          </label>
          <Select id="filters-app" value={appFilter} onChange={(e) => setAppFilter(e.target.value)}>
            <option value="all">{t('common.all')}</option>
            {appList.map((app) => (
              <option key={app} value={app}>
                {app}
              </option>
            ))}
          </Select>
        </div>

        {/* Importance filter */}
        <div>
          <label
            htmlFor="filters-importance"
            className={`mb-2 block ${typography.weight.medium} text-content-strong text-sm`}
          >
            {t('timeline.importance')}
          </label>
          <Select
            id="filters-importance"
            value={importanceFilter}
            onChange={(e) => setImportanceFilter(e.target.value as ImportanceFilter)}
          >
            <option value="all">{t('common.all')}</option>
            <option value="high">{t('timeline.high')}</option>
            <option value="medium">{t('timeline.medium')}</option>
            <option value="low">{t('timeline.low')}</option>
          </Select>
        </div>

        {/* Tag filter */}
        {allTags.length > 0 && (
          <div>
            <label
              htmlFor="filters-tag"
              className={`mb-2 block ${typography.weight.medium} text-content-strong text-sm`}
            >
              {t('timeline.tag')}
            </label>
            <Select
              id="filters-tag"
              value={tagFilter === 'all' ? 'all' : String(tagFilter)}
              onChange={(e) => setTagFilter(e.target.value === 'all' ? 'all' : Number(e.target.value))}
            >
              <option value="all">{t('common.all')}</option>
              {allTags.map((tag) => (
                <option key={tag.id} value={tag.id}>
                  {tag.name}
                </option>
              ))}
            </Select>
          </div>
        )}

        {/* Filter summary */}
        <div className="border-muted border-t pt-4 text-content-secondary text-sm">
          {t('timeline.filterSummary', {
            shown: filteredFrames.length,
            total: frames.length,
            defaultValue: 'Showing {{shown}} of {{total}} frames',
          })}
        </div>
      </div>
    </Card>
  )
}
