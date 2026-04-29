/**
 * Timeline layout — manages frames query, tags query, filter state, and view mode.
 * Child routes receive data via Outlet context.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Outlet, useSearchParams } from 'react-router-dom'
import {
  addTagToFrame,
  batchAddTag,
  type Frame,
  fetchFrames,
  fetchFrameTags,
  fetchSettings,
  fetchTags,
  removeTagFromFrame,
} from '../../api/client'
import { isStandaloneModeEnabled } from '../../api/standalone'
import DateRangePicker from '../../components/DateRangePicker'
import Lightbox from '../../components/Lightbox'
import { Skeleton } from '../../components/ui'
import { useKeyboardShortcuts } from '../../hooks/useKeyboardShortcuts'
import { addToast } from '../../hooks/useToast'
import { colors, typography } from '../../styles/tokens'
import { resolveImageUrl } from '../../utils/api-base'
import { cn } from '../../utils/cn'

export type ViewMode = 'grid' | 'list'
export type ImportanceFilter = 'all' | 'high' | 'medium' | 'low'

export interface TimelineContext {
  frames: Frame[]
  filteredFrames: Frame[]
  pagination: { total: number; has_more: boolean } | undefined
  page: number
  setPage: (updater: number | ((prev: number) => number)) => void
  pageSize: number
  allTags: Awaited<ReturnType<typeof fetchTags>>
  selectedFrame: Frame | null
  setSelectedFrame: React.Dispatch<React.SetStateAction<Frame | null>>
  selectedIndex: number
  setSelectedIndex: React.Dispatch<React.SetStateAction<number>>
  selectedFrameTags: Awaited<ReturnType<typeof fetchFrameTags>>
  addTagMutation: ReturnType<typeof useMutation<void, Error, { frameId: number; tagId: number }>>
  removeTagMutation: ReturnType<typeof useMutation<void, Error, { frameId: number; tagId: number }>>
  batchTagMutation: ReturnType<
    typeof useMutation<{ tagged_count: number }, Error, { frameIds: number[]; tagId: number }>
  >
  viewMode: ViewMode
  setViewMode: React.Dispatch<React.SetStateAction<ViewMode>>
  appFilter: string
  setAppFilter: React.Dispatch<React.SetStateAction<string>>
  importanceFilter: ImportanceFilter
  setImportanceFilter: React.Dispatch<React.SetStateAction<ImportanceFilter>>
  tagFilter: number | 'all'
  setTagFilter: React.Dispatch<React.SetStateAction<number | 'all'>>
  appList: string[]
  selectMode: boolean
  setSelectMode: React.Dispatch<React.SetStateAction<boolean>>
  selectedFrames: Set<number>
  setSelectedFrames: React.Dispatch<React.SetStateAction<Set<number>>>
  toggleFrameSelection: (frameId: number) => void
  exitSelectMode: () => void
  selectAllFiltered: () => void
  selectFrame: (frame: Frame, index: number) => void
  goToPrev: () => void
  goToNext: () => void
  openLightbox: () => void
  handleCopyOcr: () => Promise<void>
  lightboxOpen: boolean
  setLightboxOpen: React.Dispatch<React.SetStateAction<boolean>>
  standaloneMode: boolean
  /**
   * Undefined while `/settings` is still loading; otherwise the last known
   * value of `capture_enabled`. Consumed by the AllFrames empty-state branch
   * that decides between "waiting" vs "capture disabled" messaging.
   */
  captureEnabled: boolean | undefined
}

export default function TimelineLayout() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [selectedFrame, setSelectedFrame] = useState<Frame | null>(null)
  const [selectedIndex, setSelectedIndex] = useState<number>(-1)
  const [searchParams, setSearchParams] = useSearchParams()
  const page = Number(searchParams.get('page') || '0')
  const setPage = useCallback(
    (updater: number | ((prev: number) => number)) => {
      const next = typeof updater === 'function' ? updater(page) : updater
      setSearchParams(
        (prev) => {
          const p = new URLSearchParams(prev)
          if (next === 0) p.delete('page')
          else p.set('page', String(next))
          return p
        },
        { replace: true },
      )
    },
    [page, setSearchParams],
  )
  const [dateRange, setDateRange] = useState<{ from?: string; to?: string }>({})
  const [lightboxOpen, setLightboxOpen] = useState(false)
  const [viewMode, setViewMode] = useState<ViewMode>('grid')
  const [appFilter, setAppFilter] = useState<string>('all')
  const [importanceFilter, setImportanceFilter] = useState<ImportanceFilter>('all')
  const [tagFilter, setTagFilter] = useState<number | 'all'>('all')
  const pageSize = 50
  const standaloneMode = isStandaloneModeEnabled()
  const [selectMode, setSelectMode] = useState(false)
  const [selectedFrames, setSelectedFrames] = useState<Set<number>>(new Set())

  const toggleFrameSelection = useCallback((frameId: number) => {
    setSelectedFrames((prev) => {
      const next = new Set(prev)
      if (next.has(frameId)) next.delete(frameId)
      else next.add(frameId)
      return next
    })
  }, [])

  const exitSelectMode = useCallback(() => {
    setSelectMode(false)
    setSelectedFrames(new Set())
  }, [])

  const batchTagMutation = useMutation({
    mutationFn: ({ frameIds, tagId }: { frameIds: number[]; tagId: number }) => batchAddTag(frameIds, tagId),
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: ['frames'] })
      queryClient.invalidateQueries({ queryKey: ['frame-tags'] })
      addToast('success', t('timeline.batchTagged', { count: data.tagged_count }))
      exitSelectMode()
    },
  })

  const handleRangeChange = useCallback(
    (from: string | undefined, to: string | undefined) => {
      setDateRange({ from, to })
      setPage(0)
    },
    [setPage],
  )

  const { data: allTags = [] } = useQuery({
    queryKey: ['tags'],
    queryFn: fetchTags,
  })

  const { data: settings } = useQuery({
    queryKey: ['settings'],
    queryFn: fetchSettings,
    staleTime: Number.POSITIVE_INFINITY,
  })

  const { data: selectedFrameTags = [] } = useQuery({
    queryKey: ['frame-tags', selectedFrame?.id],
    queryFn: () => (selectedFrame ? fetchFrameTags(selectedFrame.id) : Promise.resolve([])),
    enabled: !!selectedFrame,
  })

  const addTagMutation = useMutation({
    mutationFn: ({ frameId, tagId }: { frameId: number; tagId: number }) => addTagToFrame(frameId, tagId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['frame-tags', selectedFrame?.id] })
    },
  })

  const removeTagMutation = useMutation({
    mutationFn: ({ frameId, tagId }: { frameId: number; tagId: number }) => removeTagFromFrame(frameId, tagId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['frame-tags', selectedFrame?.id] })
    },
  })

  const { data: response, isLoading } = useQuery({
    queryKey: ['frames', page, dateRange.from, dateRange.to],
    queryFn: () => fetchFrames(dateRange.from, dateRange.to, pageSize, page * pageSize),
    staleTime: 120_000,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    placeholderData: (prev) => prev,
  })

  const frames = response?.data ?? []
  const pagination = response?.pagination

  const filteredFrames = useMemo(() => {
    return frames.filter((frame) => {
      if (appFilter !== 'all' && frame.app_name !== appFilter) return false
      if (importanceFilter === 'high' && frame.importance < 0.7) return false
      if (importanceFilter === 'medium' && (frame.importance < 0.4 || frame.importance >= 0.7)) return false
      if (importanceFilter === 'low' && frame.importance >= 0.4) return false
      if (tagFilter !== 'all' && !(frame.tag_ids ?? []).includes(tagFilter as number)) return false
      return true
    })
  }, [frames, appFilter, importanceFilter, tagFilter])

  const selectAllFiltered = useCallback(() => {
    setSelectedFrames(new Set(filteredFrames.map((f) => f.id)))
  }, [filteredFrames])

  const appList = useMemo(() => {
    const apps = new Set(frames.map((f) => f.app_name))
    return Array.from(apps).sort()
  }, [frames])

  const selectFrame = useCallback((frame: Frame, index: number) => {
    setSelectedFrame(frame)
    setSelectedIndex(index)
  }, [])

  const goToPrev = useCallback(() => {
    if (selectedIndex > 0) {
      const newIndex = selectedIndex - 1
      setSelectedFrame(filteredFrames[newIndex])
      setSelectedIndex(newIndex)
    }
  }, [selectedIndex, filteredFrames])

  const goToNext = useCallback(() => {
    if (selectedIndex < filteredFrames.length - 1) {
      const newIndex = selectedIndex + 1
      setSelectedFrame(filteredFrames[newIndex])
      setSelectedIndex(newIndex)
    }
  }, [selectedIndex, filteredFrames])

  const openLightbox = useCallback(() => {
    if (selectedFrame?.image_url) {
      setLightboxOpen(true)
    }
  }, [selectedFrame])

  const handleCopyOcr = useCallback(async () => {
    const ocrText = selectedFrame?.ocr_text
    if (!ocrText) {
      return
    }

    try {
      if (!navigator.clipboard?.writeText) {
        throw new Error('Clipboard API is unavailable')
      }

      await navigator.clipboard.writeText(ocrText)
      addToast('success', t('timeline.ocrCopied'))
    } catch {
      addToast('error', t('timeline.ocrCopyFailed'))
    }
  }, [selectedFrame, t])

  useKeyboardShortcuts({
    onEscape: () => {
      if (lightboxOpen) {
        setLightboxOpen(false)
      } else if (selectMode) {
        exitSelectMode()
      } else {
        setSelectedFrame(null)
        setSelectedIndex(-1)
      }
    },
    onArrowLeft: goToPrev,
    onArrowRight: goToNext,
    onEnter: openLightbox,
    onSpace: openLightbox,
  })

  if (isLoading) {
    const skeletonIds = Array.from({ length: 8 }, (_, index) => `timeline-skeleton-${index}`)

    return (
      <div className="min-h-full space-y-6 p-6">
        <Skeleton className="h-8 w-40" />
        <div className="flex gap-4">
          <Skeleton className="h-10 w-40" />
          <Skeleton className="h-10 w-40" />
          <Skeleton className="h-10 w-40" />
        </div>
        <div className="grid grid-cols-2 gap-4 md:grid-cols-3 lg:grid-cols-4">
          {skeletonIds.map((skeletonId) => (
            <Skeleton key={skeletonId} className="h-40 rounded-lg" />
          ))}
        </div>
      </div>
    )
  }

  // Empty-state UX is owned by AllFrames (the defaultChild) so that this
  // layout can always render <Outlet> — otherwise the index
  // <Navigate to="all" replace /> emitted by RouteRenderer would never fire
  // and `/timeline` would get stuck without redirecting to `/timeline/all`.
  // Same bug class as the AuditLayout empty-state regression.
  const ctx: TimelineContext = {
    frames,
    filteredFrames,
    pagination,
    page,
    setPage,
    pageSize,
    allTags,
    selectedFrame,
    setSelectedFrame,
    selectedIndex,
    setSelectedIndex,
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
    lightboxOpen,
    setLightboxOpen,
    standaloneMode,
    captureEnabled: settings?.capture_enabled,
  }

  return (
    <div className="min-h-full space-y-6 p-6">
      {/* Header */}
      <div className="flex flex-col justify-between gap-4 md:flex-row md:items-center">
        <div className="flex items-center space-x-4">
          <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('timeline.title')}</h1>
          <span className="text-content-secondary">
            {pagination ? `${pagination.total}${t('timeline.captures')}` : `${frames.length}${t('timeline.captures')}`}
            {filteredFrames.length !== frames.length && ` (${filteredFrames.length}${t('timeline.showing')})`}
          </span>
        </div>
        <DateRangePicker onRangeChange={handleRangeChange} />
      </div>

      <Outlet context={ctx} />

      {/* Lightbox */}
      {lightboxOpen && selectedFrame?.image_url && (
        <Lightbox
          imageUrl={resolveImageUrl(selectedFrame.image_url) ?? selectedFrame.image_url}
          alt={selectedFrame.window_title}
          onClose={() => setLightboxOpen(false)}
          onPrev={selectedIndex > 0 ? goToPrev : undefined}
          onNext={selectedIndex < filteredFrames.length - 1 ? goToNext : undefined}
          hasPrev={selectedIndex > 0}
          hasNext={selectedIndex < filteredFrames.length - 1}
        />
      )}
    </div>
  )
}
