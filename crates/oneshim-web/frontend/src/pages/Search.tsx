/**
 *
 */

import { useQuery } from '@tanstack/react-query'
import { Brain, Clock, FileText, Search as SearchIcon } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { fetchSemanticSearch, fetchTags, type SearchResult, search } from '../api/client'
import type { SemanticSearchResult } from '../api/contracts'
import { TagBadge } from '../components/TagBadge'
import { Badge, Button, Card, EmptyState, Input, Spinner } from '../components/ui'
import { colors, iconSize, motion, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { escapeRegex, formatDateTime } from '../utils/formatters'

function highlightText(text: string, query: string): JSX.Element {
  if (!query || !text) return <>{text}</>

  const parts = text.split(new RegExp(`(${escapeRegex(query)})`, 'gi'))
  const elements: React.ReactNode[] = []
  let offset = 0
  for (const part of parts) {
    const key = `${offset}-${part.length}`
    if (part.toLowerCase() === query.toLowerCase()) {
      elements.push(
        <mark key={key} className="rounded bg-semantic-warning/25 px-0.5">
          {part}
        </mark>,
      )
    } else {
      elements.push(<span key={key}>{part}</span>)
    }
    offset += part.length
  }
  return <>{elements}</>
}

type SearchType = 'all' | 'frames' | 'events'
type SearchMode = 'text' | 'semantic'

export default function Search() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const initialQuery = searchParams.get('q') || ''
  const initialTagIds = searchParams.get('tags')?.split(',').map(Number).filter(Boolean) || []

  const [inputValue, setInputValue] = useState(initialQuery)
  const [searchQuery, setSearchQuery] = useState(initialQuery)
  const [searchMode, setSearchMode] = useState<SearchMode>('text')
  const [searchType, setSearchType] = useState<SearchType>('all')
  const [selectedTagIds, setSelectedTagIds] = useState<number[]>(initialTagIds)
  const [page, setPage] = useState(0)
  const pageSize = 20

  const { data: allTags = [] } = useQuery({
    queryKey: ['tags'],
    queryFn: fetchTags,
  })

  const hasSearchCriteria =
    searchMode === 'text' ? searchQuery.length > 0 || selectedTagIds.length > 0 : searchQuery.length > 0

  const {
    data: response,
    isLoading: isTextLoading,
    error: textError,
  } = useQuery({
    queryKey: ['search', searchQuery, searchType, selectedTagIds, page],
    queryFn: () =>
      search({
        query: searchQuery,
        searchType,
        tagIds: selectedTagIds.length > 0 ? selectedTagIds : undefined,
        limit: pageSize,
        offset: page * pageSize,
      }),
    enabled: hasSearchCriteria && searchMode === 'text',
  })

  const {
    data: semanticResults,
    isLoading: isSemanticLoading,
    error: semanticError,
  } = useQuery({
    queryKey: ['semantic-search', searchQuery],
    queryFn: () => fetchSemanticSearch(searchQuery, pageSize),
    enabled: hasSearchCriteria && searchMode === 'semantic' && searchQuery.length > 0,
  })

  const isLoading = searchMode === 'text' ? isTextLoading : isSemanticLoading
  const error = searchMode === 'text' ? textError : semanticError

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault()
    const trimmed = inputValue.trim()
    if (trimmed || selectedTagIds.length > 0) {
      setSearchQuery(trimmed)
      const params: Record<string, string> = {}
      if (trimmed) params.q = trimmed
      if (selectedTagIds.length > 0) params.tags = selectedTagIds.join(',')
      setSearchParams(params)
      setPage(0)
    }
  }

  const handleTypeChange = (type: SearchType) => {
    setSearchType(type)
    setPage(0)
  }

  const handleTagToggle = (tagId: number) => {
    setSelectedTagIds((prev) => (prev.includes(tagId) ? prev.filter((id) => id !== tagId) : [...prev, tagId]))
    setPage(0)
  }

  const handleClearTags = () => {
    setSelectedTagIds([])
    setPage(0)
  }

  return (
    <div className="min-h-full space-y-6 p-6">
      {/* UI note */}
      <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('search.title')}</h1>

      {/* Search mode toggle + search form */}
      <div className="flex items-center gap-3">
        <div className="flex rounded-lg border border-DEFAULT bg-surface-muted p-0.5">
          <button
            type="button"
            data-testid="mode-text"
            className={cn(
              'flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm',
              motion.colors,
              searchMode === 'text' ? 'bg-surface text-content shadow-sm' : 'text-content-secondary hover:text-content',
            )}
            onClick={() => {
              setSearchMode('text')
              setPage(0)
            }}
          >
            <SearchIcon className="h-3.5 w-3.5" />
            {t('search.textSearch')}
          </button>
          <button
            type="button"
            data-testid="mode-semantic"
            className={cn(
              'flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm',
              motion.colors,
              searchMode === 'semantic'
                ? 'bg-surface text-content shadow-sm'
                : 'text-content-secondary hover:text-content',
            )}
            onClick={() => {
              setSearchMode('semantic')
              setPage(0)
            }}
          >
            <Brain className="h-3.5 w-3.5" />
            {t('search.semantic')}
          </button>
        </div>
      </div>

      <form id="section-recent" onSubmit={handleSearch} className="flex gap-2">
        <Input
          data-testid="search-input"
          type="text"
          value={inputValue}
          onChange={(e) => setInputValue(e.target.value)}
          placeholder={searchMode === 'semantic' ? t('search.semanticPlaceholder') : t('search.placeholder')}
          className="flex-1"
        />
        <Button type="submit" variant="primary" size="lg">
          {t('common.search')}
        </Button>
      </form>

      {/* Type filters + tag filters (text mode only) */}
      {searchMode === 'text' && (
        <>
          <div id="section-tags" className="flex flex-wrap items-center gap-4">
            <div className="flex space-x-2">
              {(['all', 'frames', 'events'] as SearchType[]).map((type) => (
                <Button
                  key={type}
                  data-testid={`filter-${type}`}
                  variant={searchType === type ? 'primary' : 'secondary'}
                  size="sm"
                  onClick={() => handleTypeChange(type)}
                >
                  {type === 'all' ? t('common.all') : type === 'frames' ? t('search.frames') : t('search.events')}
                </Button>
              ))}
            </div>

            <div className="h-8 w-px bg-hover" />

            <div className="flex flex-wrap items-center gap-2">
              <span className="text-content-secondary text-sm">{t('search.filterByTags')}:</span>
              {allTags.map((tag) => (
                <TagBadge
                  key={tag.id}
                  name={tag.name}
                  color={tag.color}
                  size="sm"
                  selected={selectedTagIds.includes(tag.id)}
                  onClick={() => handleTagToggle(tag.id)}
                />
              ))}
              {selectedTagIds.length > 0 && (
                <Button variant="ghost" size="sm" onClick={handleClearTags}>
                  {t('search.clearTags')}
                </Button>
              )}
            </div>
          </div>

          {selectedTagIds.length > 0 && (
            <div className="text-content-secondary text-sm">
              {t('search.selectedTags')}:{' '}
              {allTags
                .filter((tag) => selectedTagIds.includes(tag.id))
                .map((tag) => tag.name)
                .join(', ')}
            </div>
          )}
        </>
      )}

      {/* UI note */}
      {isLoading && (
        <div className="flex h-32 items-center justify-center">
          <Spinner size="lg" className="text-brand-text" />
          <span className="ml-3 text-content-secondary">{t('common.loading')}</span>
        </div>
      )}

      {error && (
        <Card variant="danger" padding="md">
          <p className="text-semantic-error">{t('search.searchError')}</p>
        </Card>
      )}

      {/* Text search results */}
      {searchMode === 'text' && response && (
        <>
          <div className="text-content-secondary">
            {response.query && (
              <>
                "<span className="text-content">{response.query}</span>"{' '}
              </>
            )}
            {t('search.results')}: <span className="text-brand-text">{response.total}</span>
            {t('search.resultCount')}
          </div>

          {response.results.length > 0 ? (
            <div className="space-y-3">
              {response.results.map((result) => (
                <SearchResultCard
                  key={`${result.result_type}-${result.id}`}
                  result={result}
                  query={searchQuery}
                  onTagClick={handleTagToggle}
                  selectedTagIds={selectedTagIds}
                />
              ))}
            </div>
          ) : (
            <EmptyState
              icon={<SearchIcon className="h-8 w-8" aria-hidden="true" />}
              title={t('search.noResults')}
              description={t('search.searchHint')}
            />
          )}

          {response.total > pageSize && (
            <div className="flex items-center justify-center space-x-4">
              <Button
                variant="secondary"
                size="md"
                onClick={() => setPage((p) => Math.max(0, p - 1))}
                disabled={page === 0}
              >
                {t('common.prev')}
              </Button>
              <span className="text-content-secondary">
                {page + 1} / {Math.ceil(response.total / pageSize)} {t('common.page')}
              </span>
              <Button
                variant="secondary"
                size="md"
                onClick={() => setPage((p) => p + 1)}
                disabled={(page + 1) * pageSize >= response.total}
              >
                {t('common.next')}
              </Button>
            </div>
          )}
        </>
      )}

      {/* Semantic search results */}
      {searchMode === 'semantic' && semanticResults && (
        <>
          <div className="text-content-secondary">
            "<span className="text-content">{searchQuery}</span>" {t('search.results')}:{' '}
            <span className="text-brand-text">{semanticResults.length}</span>
            {t('search.resultCount')}
          </div>

          {semanticResults.length > 0 ? (
            <div className="space-y-3">
              {semanticResults.map((result) => (
                <SemanticResultCard key={result.segment_id} result={result} />
              ))}
            </div>
          ) : (
            <EmptyState
              icon={<Brain className="h-8 w-8" aria-hidden="true" />}
              title={t('search.noResults')}
              description={t('search.searchHint')}
            />
          )}
        </>
      )}

      {/* UI note */}
      {!hasSearchCriteria && (
        <EmptyState
          icon={<SearchIcon className="h-8 w-8" aria-hidden="true" />}
          title={t('search.enterQuery')}
          description={t('search.searchHint')}
          action={{ label: t('search.browseTimeline'), onClick: () => navigate('/timeline/all') }}
        />
      )}
    </div>
  )
}

interface SearchResultCardProps {
  result: SearchResult
  query: string
  onTagClick: (tagId: number) => void
  selectedTagIds: number[]
}

function SearchResultCard({ result, query, onTagClick, selectedTagIds }: SearchResultCardProps) {
  const { t } = useTranslation()
  const isFrame = result.result_type === 'frame'

  return (
    <Card padding="md" className="flex gap-4">
      {/* UI note */}
      {isFrame && result.image_url && (
        <div className="h-16 w-24 flex-shrink-0 overflow-hidden rounded bg-hover">
          <img
            src={result.image_url}
            alt={result.window_title || 'Screenshot'}
            className="h-full w-full object-cover"
          />
        </div>
      )}

      {/* UI note */}
      {!isFrame && (
        <div className="flex h-16 w-16 flex-shrink-0 items-center justify-center rounded bg-hover">
          <FileText className="h-8 w-8 text-content-secondary" />
        </div>
      )}

      {/* UI note */}
      <div className="min-w-0 flex-1">
        <div className="mb-1 flex flex-wrap items-center gap-2">
          <Badge color={isFrame ? 'info' : 'primary'} size="sm">
            {isFrame ? t('search.screenshot') : t('search.event')}
          </Badge>
          <span className="text-content-secondary text-sm">{formatDateTime(result.timestamp)}</span>
          {isFrame && result.importance && (
            <span className="text-content-tertiary text-sm">
              {t('search.importance')} {((result.importance ?? 0) * 100).toFixed(0)}%
            </span>
          )}
        </div>

        <div className={`truncate ${typography.weight.medium} text-content`}>
          {result.app_name && highlightText(result.app_name, query)}
          {result.app_name && result.window_title && ' - '}
          {result.window_title && highlightText(result.window_title, query)}
        </div>

        {result.matched_text && (
          <div className="mt-1 line-clamp-2 text-content-secondary text-sm">
            {highlightText(result.matched_text, query)}
          </div>
        )}

        {result.tags && result.tags.length > 0 && (
          <div className="mt-2 flex flex-wrap gap-1">
            {result.tags.map((tag) => (
              <TagBadge
                key={tag.id}
                name={tag.name}
                color={tag.color}
                size="sm"
                selected={selectedTagIds.includes(tag.id)}
                onClick={() => onTagClick(tag.id)}
              />
            ))}
          </div>
        )}
      </div>
    </Card>
  )
}

// ── Semantic search result card ────────────────────────────────

interface SemanticResultCardProps {
  result: SemanticSearchResult
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${Math.round(secs)}s`
  if (secs < 3600) return `${Math.round(secs / 60)}m`
  const h = Math.floor(secs / 3600)
  const m = Math.round((secs % 3600) / 60)
  return m > 0 ? `${h}h ${m}m` : `${h}h`
}

function scoreColor(score: number): string {
  if (score >= 0.8) return 'text-semantic-success'
  if (score >= 0.5) return 'text-semantic-warning'
  return 'text-content-secondary'
}

function SemanticResultCard({ result }: SemanticResultCardProps) {
  const { t } = useTranslation()
  const scorePercent = Math.round(result.score * 100)

  return (
    <Card padding="md" className="flex gap-4">
      {/* Score indicator */}
      <div className="flex flex-shrink-0 flex-col items-center justify-center gap-1">
        <span className={cn('text-xl', typography.weight.bold, scoreColor(result.score))}>{scorePercent}%</span>
        <span className="text-content-tertiary text-xs">{t('search.relevance')}</span>
      </div>

      {/* Content */}
      <div className="min-w-0 flex-1">
        <div className="mb-1 flex flex-wrap items-center gap-2">
          <Badge color="primary" size="sm">
            {result.content_type}
          </Badge>
          {result.regime_label && (
            <Badge color="info" size="sm">
              {result.regime_label}
            </Badge>
          )}
          {result.duration_secs != null && result.duration_secs > 0 && (
            <span className="flex items-center gap-1 text-content-tertiary text-xs">
              <Clock className={iconSize.xs} />
              {formatDuration(result.duration_secs)}
            </span>
          )}
          {result.timestamp && (
            <span className="text-content-secondary text-sm">{formatDateTime(result.timestamp)}</span>
          )}
        </div>

        {result.content_label && (
          <div className={cn('truncate text-content', typography.weight.medium)}>{result.content_label}</div>
        )}

        {result.llm_summary && (
          <div className="mt-1 line-clamp-2 text-content-secondary text-sm">{result.llm_summary}</div>
        )}

        {!result.llm_summary && result.original_text && (
          <div className="mt-1 line-clamp-2 text-content-secondary text-sm">{result.original_text}</div>
        )}

        {/* Score breakdown */}
        <div className="mt-2 flex items-center gap-3 text-content-tertiary text-xs">
          <span>
            {t('search.similarity')}: {Math.round(result.similarity * 100)}%
          </span>
          <span>
            {t('search.timeDecay')}: {result.time_decay.toFixed(2)}
          </span>
        </div>
      </div>
    </Card>
  )
}
