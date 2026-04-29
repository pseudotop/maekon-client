/**
 * Timeline all-frames section — grid/list view with inline filter bar,
 * pagination, lightbox, tag management, selection mode, and detail panel.
 *
 * Owns the Timeline empty state (frames.length === 0) so that TimelineLayout
 * can always render <Outlet> and the `/timeline` → `/timeline/all` index
 * redirect keeps firing even when no frames have been captured yet.
 * Matches the AuditLayout empty-state-in-child pattern.
 */

import { Camera, CheckSquare, Copy, Square } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { TagBadge } from '../../components/TagBadge'
import { TagInput } from '../../components/TagInput'
import { Badge, Button, Card, CardTitle, EmptyState, Select } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { iconSize, interaction, motion, typography } from '../../styles/tokens'
import { resolveImageUrl } from '../../utils/api-base'
import { cn } from '../../utils/cn'
import { formatDate, formatTime } from '../../utils/formatters'
import type { ImportanceFilter, TimelineContext } from './TimelineLayout'

function getImportanceBadge(importance: number) {
  if (importance >= 0.7) return { color: 'success' as const, label: `${(importance * 100).toFixed(0)}%` }
  if (importance >= 0.4) return { color: 'warning' as const, label: `${(importance * 100).toFixed(0)}%` }
  return { color: 'default' as const, label: `${(importance * 100).toFixed(0)}%` }
}

function getFrameButtonLabel(
  frame: TimelineContext['frames'][number],
  locale: string,
  selected: boolean,
  t: ReturnType<typeof useTranslation>['t'],
) {
  const importance = `${(frame.importance * 100).toFixed(0)}%`
  return t('timeline.frameButtonLabel', {
    app: frame.app_name,
    window: frame.window_title,
    importance,
    date: formatDate(frame.timestamp, locale),
    time: formatTime(frame.timestamp, locale),
    state: selected ? t('timeline.frameSelected') : t('timeline.frameNotSelected'),
  })
}

export default function AllFrames() {
  const { t, i18n } = useTranslation()
  const locale = i18n.resolvedLanguage ?? i18n.language
  const navigate = useNavigate()
  const {
    frames,
    filteredFrames,
    pagination,
    page,
    setPage,
    pageSize,
    allTags,
    selectedFrame,
    selectedIndex,
    selectedFrameTags,
    addTagMutation,
    removeTagMutation,
    batchTagMutation,
    viewMode,
    setViewMode,
    appFilter,
    setAppFilter,
    importanceFilter,
    setImportanceFilter,
    tagFilter,
    setTagFilter,
    appList,
    selectMode,
    setSelectMode,
    selectedFrames,
    setSelectedFrames,
    toggleFrameSelection,
    exitSelectMode,
    selectAllFiltered,
    selectFrame,
    goToPrev,
    goToNext,
    openLightbox,
    handleCopyOcr,
    setLightboxOpen,
    standaloneMode,
    captureEnabled,
  } = useTypedOutletContext<TimelineContext>('Timeline')

  if (frames.length === 0) {
    const emptyState = standaloneMode
      ? {
          title: t('emptyState.timelineStandalone.title'),
          description: t('emptyState.timelineStandalone.description'),
          action: undefined as { label: string; onClick: () => void } | undefined,
        }
      : captureEnabled === false
        ? {
            title: t('emptyState.timeline.title'),
            description: t('emptyState.timeline.description'),
            action: {
              label: t('emptyState.timeline.action'),
              onClick: () => navigate('/settings/monitoring'),
            },
          }
        : {
            title: t('emptyState.timelineWaiting.title'),
            description: t('emptyState.timelineWaiting.description'),
            action: undefined as { label: string; onClick: () => void } | undefined,
          }

    return (
      <EmptyState
        icon={<Camera className="h-8 w-8" />}
        title={emptyState.title}
        description={emptyState.description}
        action={emptyState.action}
      />
    )
  }

  return (
    <>
      {/* Filter bar */}
      <Card id="section-filters" variant="default" padding="md">
        <div className="flex flex-wrap items-center gap-4">
          {/* App filter */}
          <div id="section-by-app" className="flex items-center gap-2">
            <label htmlFor="timeline-app-filter" className="text-content-secondary text-sm">
              {t('timeline.app')}:
            </label>
            <Select
              id="timeline-app-filter"
              value={appFilter}
              onChange={(e) => setAppFilter(e.target.value)}
              selectSize="sm"
            >
              <option value="all">{t('common.all')}</option>
              {appList.map((app) => (
                <option key={app} value={app}>
                  {app}
                </option>
              ))}
            </Select>
          </div>

          {/* Importance filter */}
          <div id="section-by-importance" className="flex items-center gap-2">
            <label htmlFor="timeline-importance-filter" className="text-content-secondary text-sm">
              {t('timeline.importance')}:
            </label>
            <Select
              id="timeline-importance-filter"
              value={importanceFilter}
              onChange={(e) => setImportanceFilter(e.target.value as ImportanceFilter)}
              selectSize="sm"
            >
              <option value="all">{t('common.all')}</option>
              <option value="high">{t('timeline.high')}</option>
              <option value="medium">{t('timeline.medium')}</option>
              <option value="low">{t('timeline.low')}</option>
            </Select>
          </div>

          {/* Tag filter */}
          {allTags.length > 0 && (
            <div id="section-by-tag" className="flex items-center gap-2">
              <label htmlFor="timeline-tag-filter" className="text-content-secondary text-sm">
                {t('timeline.tag')}:
              </label>
              <Select
                id="timeline-tag-filter"
                value={tagFilter === 'all' ? 'all' : String(tagFilter)}
                onChange={(e) => setTagFilter(e.target.value === 'all' ? 'all' : Number(e.target.value))}
                selectSize="sm"
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

          {/* Select mode toggle */}
          <div className="ml-auto flex items-center gap-2">
            <Button
              data-testid="toggle-select-mode"
              variant={selectMode ? 'primary' : 'secondary'}
              size="sm"
              onClick={() => (selectMode ? exitSelectMode() : setSelectMode(true))}
            >
              <CheckSquare className={cn('mr-1', iconSize.base)} />
              {selectMode ? t('timeline.cancelSelect') : t('timeline.select')}
            </Button>
          </div>

          {/* View mode toggle */}
          <div className="flex items-center gap-1">
            <Button
              data-testid="view-grid"
              variant={viewMode === 'grid' ? 'primary' : 'secondary'}
              size="icon"
              onClick={() => setViewMode('grid')}
              title={t('timeline.gridView')}
            >
              <svg
                className={`${iconSize.md}`}
                fill="none"
                stroke="currentColor"
                viewBox="0 0 24 24"
                aria-hidden="true"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  strokeWidth={2}
                  d="M4 6a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2H6a2 2 0 01-2-2V6zm10 0a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2h-2a2 2 0 01-2-2V6zM4 16a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2H6a2 2 0 01-2-2v-2zm10 0a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2h-2a2 2 0 01-2-2v-2z"
                />
              </svg>
            </Button>
            <Button
              data-testid="view-list"
              variant={viewMode === 'list' ? 'primary' : 'secondary'}
              size="icon"
              onClick={() => setViewMode('list')}
              title={t('timeline.listView')}
            >
              <svg
                className={`${iconSize.md}`}
                fill="none"
                stroke="currentColor"
                viewBox="0 0 24 24"
                aria-hidden="true"
              >
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 6h16M4 12h16M4 18h16" />
              </svg>
            </Button>
          </div>

          {/* Keyboard hints */}
          <div className="hidden items-center text-content-tertiary text-xs md:flex">
            <kbd className="rounded bg-hover px-1.5 py-0.5 text-content-strong">← →</kbd>
            <span className="ml-1">{t('timeline.move')}</span>
            <span className="mx-2">|</span>
            <kbd className="rounded bg-hover px-1.5 py-0.5 text-content-strong">Enter</kbd>
            <span className="ml-1">{t('timeline.enlarge')}</span>
          </div>
        </div>
      </Card>

      {/* Grid view */}
      {viewMode === 'grid' && (
        <Card id="section-all" variant="default" padding="md">
          <div className="grid grid-cols-4 gap-2 sm:grid-cols-6 md:grid-cols-8 lg:grid-cols-10">
            {filteredFrames.map((frame, index) => {
              const isSelected = selectMode ? selectedFrames.has(frame.id) : selectedFrame?.id === frame.id
              const frameButtonLabel = getFrameButtonLabel(frame, locale, isSelected, t)
              return (
                <button
                  type="button"
                  key={frame.id}
                  data-testid={`frame-card-${frame.id}`}
                  aria-label={frameButtonLabel}
                  title={frameButtonLabel}
                  aria-pressed={isSelected}
                  onClick={() => (selectMode ? toggleFrameSelection(frame.id) : selectFrame(frame, index))}
                  onDoubleClick={() => {
                    if (!selectMode) {
                      selectFrame(frame, index)
                      if (frame.image_url) setLightboxOpen(true)
                    }
                  }}
                  className={cn(
                    `relative aspect-video overflow-hidden rounded border-2 bg-hover ${motion.all} hover:scale-105`,
                    interaction.focusRing,
                    selectMode && selectedFrames.has(frame.id)
                      ? 'border-brand-signal ring-2 ring-brand-signal/50'
                      : selectedFrame?.id === frame.id && !selectMode
                        ? 'border-brand-signal ring-2 ring-brand-signal/50'
                        : 'border-transparent hover:border-strong',
                  )}
                >
                  {frame.image_url ? (
                    <img
                      src={resolveImageUrl(frame.image_url) ?? undefined}
                      alt={frame.window_title}
                      className="h-full w-full object-cover"
                      loading="lazy"
                    />
                  ) : (
                    <div className="flex h-full w-full items-center justify-center text-content-tertiary text-xs">
                      {t('timeline.noImage')}
                    </div>
                  )}
                  {selectMode && (
                    <div className="absolute top-1 left-1">
                      {selectedFrames.has(frame.id) ? (
                        <CheckSquare className={cn(iconSize.md, 'text-brand-signal drop-shadow')} />
                      ) : (
                        <Square className={cn(iconSize.md, 'text-content-inverse drop-shadow')} />
                      )}
                    </div>
                  )}
                </button>
              )
            })}
          </div>
          {filteredFrames.length === 0 && (
            <div className="py-8 text-center text-content-secondary">
              {frames.length === 0 ? t('timeline.noFrames') : t('timeline.noFilterMatch')}
            </div>
          )}
        </Card>
      )}

      {/* List view */}
      {viewMode === 'list' && (
        <Card variant="default" padding="none">
          <div className="divide-y divide-border-muted">
            {filteredFrames.map((frame, index) => {
              const badge = getImportanceBadge(frame.importance)
              return (
                <button
                  type="button"
                  key={frame.id}
                  data-testid={`frame-row-${frame.id}`}
                  onClick={() => (selectMode ? toggleFrameSelection(frame.id) : selectFrame(frame, index))}
                  onDoubleClick={() => {
                    if (!selectMode) {
                      selectFrame(frame, index)
                      if (frame.image_url) setLightboxOpen(true)
                    }
                  }}
                  className={cn(
                    `flex w-full items-center gap-4 p-3 text-left ${motion.colors}`,
                    interaction.focusRing,
                    selectMode && selectedFrames.has(frame.id)
                      ? 'bg-brand-signal/10'
                      : selectedFrame?.id === frame.id && !selectMode
                        ? 'bg-brand-signal/10'
                        : 'hover:bg-hover/50',
                  )}
                >
                  {/* Selection checkbox for list view */}
                  {selectMode && (
                    <div className="flex-shrink-0">
                      {selectedFrames.has(frame.id) ? (
                        <CheckSquare className={cn(iconSize.md, 'text-brand-signal')} />
                      ) : (
                        <Square className={cn(iconSize.md, 'text-content-tertiary')} />
                      )}
                    </div>
                  )}

                  {/* Thumbnail */}
                  <div className="h-14 w-24 flex-shrink-0 overflow-hidden rounded bg-hover">
                    {frame.image_url ? (
                      <img
                        src={resolveImageUrl(frame.image_url) ?? undefined}
                        alt={frame.window_title}
                        className="h-full w-full object-cover"
                        loading="lazy"
                      />
                    ) : (
                      <div className="flex h-full w-full items-center justify-center text-content-tertiary text-xs">
                        {t('timeline.noImage')}
                      </div>
                    )}
                  </div>

                  {/* Info */}
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className={`truncate ${typography.weight.medium} text-content text-sm`}>
                        {frame.app_name}
                      </span>
                      <Badge color={badge.color} size="sm">
                        {badge.label}
                      </Badge>
                    </div>
                    <p className="truncate text-content-secondary text-sm">{frame.window_title}</p>
                  </div>

                  {/* Timestamp */}
                  <div className="flex-shrink-0 text-right text-content-tertiary text-sm">
                    <div>{formatDate(frame.timestamp, locale)}</div>
                    <div>{formatTime(frame.timestamp, locale)}</div>
                  </div>
                </button>
              )
            })}
          </div>
          {filteredFrames.length === 0 && (
            <div className="py-8 text-center text-content-secondary">
              {frames.length === 0 ? t('timeline.noFrames') : t('timeline.noFilterMatch')}
            </div>
          )}
        </Card>
      )}

      {/* Pagination */}
      {pagination && pagination.total > pageSize && (
        <div className="flex items-center justify-center space-x-4">
          <Button variant="secondary" onClick={() => setPage((p) => Math.max(0, p - 1))} disabled={page === 0}>
            {t('common.prev')}
          </Button>
          <span className="text-content-secondary">
            {page + 1} / {Math.ceil(pagination.total / pageSize)} {t('common.page')}
          </span>
          <Button variant="secondary" onClick={() => setPage((p) => p + 1)} disabled={!pagination.has_more}>
            {t('common.next')}
          </Button>
        </div>
      )}

      {/* Detail panel */}
      {selectedFrame && (
        <Card variant="default" padding="lg">
          <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
            {/* Image preview */}
            <button
              type="button"
              className="group relative aspect-video w-full cursor-pointer overflow-hidden rounded-lg bg-surface-muted"
              onClick={openLightbox}
            >
              {selectedFrame.image_url ? (
                <>
                  <img
                    src={resolveImageUrl(selectedFrame.image_url) ?? undefined}
                    alt={selectedFrame.window_title}
                    className="h-full w-full object-contain"
                  />
                  <div
                    className={`absolute inset-0 flex items-center justify-center bg-surface-overlay/0 ${motion.colors} group-hover:bg-surface-overlay/30`}
                  >
                    <svg
                      className={`h-12 w-12 text-content-inverse opacity-0 ${motion.opacity} group-hover:opacity-100`}
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                      aria-hidden="true"
                    >
                      <path
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        strokeWidth={2}
                        d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0zM10 7v3m0 0v3m0-3h3m-3 0H7"
                      />
                    </svg>
                  </div>
                </>
              ) : (
                <div className="flex h-full w-full items-center justify-center text-content-tertiary">
                  {t('timeline.noImage')}
                </div>
              )}
            </button>

            {/* Frame details */}
            <div className="space-y-4">
              <div>
                <CardTitle className="mb-2">{t('timeline.frameInfo')}</CardTitle>
                <dl className="space-y-2">
                  <div className="flex justify-between">
                    <dt className="text-content-secondary">{t('timeline.time')}</dt>
                    <dd className="text-content">{formatTime(selectedFrame.timestamp, locale)}</dd>
                  </div>
                  <div className="flex justify-between">
                    <dt className="text-content-secondary">{t('timeline.app')}</dt>
                    <dd className="text-content">{selectedFrame.app_name}</dd>
                  </div>
                  <div className="flex justify-between">
                    <dt className="text-content-secondary">{t('timeline.windowTitle')}</dt>
                    <dd className="max-w-xs truncate text-content" title={selectedFrame.window_title}>
                      {selectedFrame.window_title}
                    </dd>
                  </div>
                  <div className="flex justify-between">
                    <dt className="text-content-secondary">{t('timeline.trigger')}</dt>
                    <dd className="text-content">{selectedFrame.trigger_type}</dd>
                  </div>
                  <div className="flex justify-between">
                    <dt className="text-content-secondary">{t('timeline.importance')}</dt>
                    <dd>
                      {(() => {
                        const badge = getImportanceBadge(selectedFrame.importance)
                        return (
                          <Badge color={badge.color} size="md">
                            {badge.label}
                          </Badge>
                        )
                      })()}
                    </dd>
                  </div>
                  <div className="flex justify-between">
                    <dt className="text-content-secondary">{t('timeline.resolution')}</dt>
                    <dd className="text-content">{selectedFrame.resolution}</dd>
                  </div>
                </dl>
              </div>

              {/* Tags */}
              <div>
                <h4 className={`mb-2 ${typography.weight.medium} text-content-secondary text-sm`}>
                  {t('timeline.tags')}
                </h4>
                <div className="space-y-2">
                  {selectedFrameTags.length > 0 && (
                    <div className="flex flex-wrap gap-1">
                      {selectedFrameTags.map((tag) => (
                        <TagBadge
                          key={tag.id}
                          name={tag.name}
                          color={tag.color}
                          size="sm"
                          onRemove={() => {
                            if (selectedFrame) {
                              removeTagMutation.mutate({ frameId: selectedFrame.id, tagId: tag.id })
                            }
                          }}
                        />
                      ))}
                    </div>
                  )}
                  <TagInput
                    selectedTags={selectedFrameTags}
                    onAddTag={(tag) => {
                      if (selectedFrame) {
                        addTagMutation.mutate({ frameId: selectedFrame.id, tagId: tag.id })
                      }
                    }}
                    onRemoveTag={(tag) => {
                      if (selectedFrame) {
                        removeTagMutation.mutate({ frameId: selectedFrame.id, tagId: tag.id })
                      }
                    }}
                    placeholder={t('timeline.addTag')}
                  />
                </div>
              </div>

              {/* OCR text */}
              {selectedFrame.ocr_text && (
                <div>
                  <div className="mb-2 flex items-center justify-between gap-2">
                    <h4 className={`${typography.weight.medium} text-content-secondary text-sm`}>
                      {t('timeline.ocrText')}
                    </h4>
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      className="gap-1"
                      onClick={() => void handleCopyOcr()}
                    >
                      <Copy className="h-3.5 w-3.5" />
                      {t('timeline.copyOcr')}
                    </Button>
                  </div>
                  <pre
                    className={`max-h-48 select-all overflow-y-auto whitespace-pre-wrap break-words rounded bg-surface-muted p-3 ${typography.family.mono} text-content-strong text-xs`}
                  >
                    {selectedFrame.ocr_text}
                  </pre>
                </div>
              )}

              {/* Navigation */}
              <div className="flex items-center justify-between border-muted border-t pt-4">
                <Button variant="secondary" onClick={goToPrev} disabled={selectedIndex <= 0}>
                  <svg
                    className={`mr-2 ${iconSize.base}`}
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                    aria-hidden="true"
                  >
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
                  </svg>
                  {t('common.prev')}
                </Button>
                <span className="text-content-secondary text-sm">
                  {selectedIndex + 1} / {filteredFrames.length}
                </span>
                <Button variant="secondary" onClick={goToNext} disabled={selectedIndex >= filteredFrames.length - 1}>
                  {t('common.next')}
                  <svg
                    className={`ml-2 ${iconSize.base}`}
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                    aria-hidden="true"
                  >
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
                  </svg>
                </Button>
              </div>
            </div>
          </div>
        </Card>
      )}

      {/* Floating batch action bar */}
      {selectMode && selectedFrames.size > 0 && (
        <div className="fixed inset-x-0 bottom-6 z-50 mx-auto flex w-fit items-center gap-3 rounded-xl border border-border-muted bg-surface-raised px-4 py-3 shadow-lg">
          <span className="text-content-secondary text-sm">
            {t('timeline.selectedCount', { count: selectedFrames.size })}
          </span>
          <Button variant="secondary" size="sm" onClick={selectAllFiltered}>
            {t('timeline.selectAll')}
          </Button>
          <Button variant="secondary" size="sm" onClick={() => setSelectedFrames(new Set())}>
            {t('timeline.clearSelection')}
          </Button>
          <TagInput
            selectedTags={[]}
            onAddTag={(tag) => {
              batchTagMutation.mutate({ frameIds: Array.from(selectedFrames), tagId: tag.id })
            }}
            onRemoveTag={() => {}}
            placeholder={t('timeline.addTag')}
          />
        </div>
      )}
    </>
  )
}
