/**
 *
 */

import { useQuery } from '@tanstack/react-query'
import { Brain, Clock, FileText, ListOrdered, Search as SearchIcon } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useSearchParams } from 'react-router-dom'
import {
  fetchSemanticSearch,
  fetchSemanticSearchCapabilities,
  fetchTags,
  type SearchResult,
  search,
  TAGS_QUERY_KEY,
} from '../api/client'
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
type SearchMode = 'text' | 'keyword' | 'semantic'

function clarificationAppsForResults(results: SearchResult[]): string[] {
  if (results.length < 3) return []

  const contexts = new Set<string>()
  const apps: string[] = []
  const seenApps = new Set<string>()

  for (const result of results) {
    const app = result.app_name?.trim() || ''
    const title = result.window_title?.trim() || result.matched_text?.trim() || ''
    if (app || title) contexts.add(`${app.toLocaleLowerCase()}\u0000${title.toLocaleLowerCase()}`)
    if (app && !seenApps.has(app)) {
      seenApps.add(app)
      apps.push(app)
    }
  }

  return contexts.size >= 3 && apps.length >= 2 ? apps : []
}

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
  const [clarificationApp, setClarificationApp] = useState<string | null>(null)
  const [page, setPage] = useState(0)
  const pageSize = 20

  const { data: allTags = [] } = useQuery({
    queryKey: TAGS_QUERY_KEY,
    queryFn: fetchTags,
  })

  // #7600: capability check so the "Semantic" toggle is honest BEFORE the user
  // searches — the shipped release build compiles the embedding pipeline out,
  // so `mode=semantic` would return HTTP 501 and `mode=hybrid` would silently
  // degrade to keyword-only results labeled "semantic". Fail-closed (treat
  // loading/error as unavailable) rather than optimistically enabling it.
  const { data: searchCapabilities } = useQuery({
    queryKey: ['semantic-search-capabilities'],
    queryFn: fetchSemanticSearchCapabilities,
    staleTime: 300_000,
    retry: 1,
  })
  const semanticAvailable = searchCapabilities?.semantic_available === true

  const hasSearchCriteria =
    searchMode === 'text' ? searchQuery.length > 0 || selectedTagIds.length > 0 : searchQuery.length > 0

  const {
    data: response,
    isLoading: isTextLoading,
    error: textError,
    refetch: refetchText,
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
    refetch: refetchSemantic,
  } = useQuery({
    queryKey: ['semantic-search', searchQuery],
    // #7600: explicitly request mode=semantic (was previously omitted, which
    // silently sent the server default mode=hybrid — so pressing "Semantic"
    // never actually ran a true vector search, it quietly returned
    // hybrid/keyword-degraded results mislabeled as semantic). Also gated on
    // `semanticAvailable` below so this never fires against a build that can
    // only return HTTP 501 for this mode.
    queryFn: () => fetchSemanticSearch(searchQuery, pageSize, 'semantic'),
    enabled: hasSearchCriteria && searchMode === 'semantic' && searchQuery.length > 0 && semanticAvailable,
  })

  // #7912 T3.3-Tier1: FTS keyword/relevance mode. Hits the storage `text_search`
  // path (SQLite FTS5 CJK-bigram + BM25) over enriched activity segments via
  // `mode=keyword`. Unlike "semantic" it never touches the embedding pipeline,
  // so it needs no capability gate — `text_search` is a required web dependency
  // and is therefore always wired. This is a distinct scope from text mode
  // (frame/event LIKE): keyword ranks activity *segments* by BM25 relevance.
  const {
    data: keywordResults,
    isLoading: isKeywordLoading,
    error: keywordError,
    refetch: refetchKeyword,
  } = useQuery({
    queryKey: ['keyword-search', searchQuery],
    queryFn: () => fetchSemanticSearch(searchQuery, pageSize, 'keyword'),
    enabled: hasSearchCriteria && searchMode === 'keyword' && searchQuery.length > 0,
  })

  const isLoading =
    searchMode === 'text' ? isTextLoading : searchMode === 'keyword' ? isKeywordLoading : isSemanticLoading
  const error = searchMode === 'text' ? textError : searchMode === 'keyword' ? keywordError : semanticError
  // #8079 (CRT-PRV-QC-CJ-00-09): the active mode's refetch so the error state
  // can offer a real retry instead of a dead-end message.
  const refetch = searchMode === 'text' ? refetchText : searchMode === 'keyword' ? refetchKeyword : refetchSemantic
  const clarificationApps = response ? clarificationAppsForResults(response.results) : []
  const hasCompleteTextResultSet = response ? response.total === response.results.length : false
  const showClarification =
    searchMode === 'text' && page === 0 && searchQuery.length > 0 && clarificationApps.length >= 2
  const activeClarificationApp =
    showClarification && hasCompleteTextResultSet && clarificationApp && clarificationApps.includes(clarificationApp)
      ? clarificationApp
      : null
  const visibleTextResults =
    response && activeClarificationApp
      ? response.results.filter((result) => result.app_name === activeClarificationApp)
      : response?.results || []

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
      setClarificationApp(null)
    }
  }

  const handleTypeChange = (type: SearchType) => {
    setSearchType(type)
    setPage(0)
    setClarificationApp(null)
  }

  const handleTagToggle = (tagId: number) => {
    setSelectedTagIds((prev) => (prev.includes(tagId) ? prev.filter((id) => id !== tagId) : [...prev, tagId]))
    setPage(0)
    setClarificationApp(null)
  }

  const handleClearTags = () => {
    setSelectedTagIds([])
    setPage(0)
    setClarificationApp(null)
  }

  return (
    <div className="min-h-full space-y-6 p-6">
      {/* UI note */}
      <h1 className={cn(typography.h1, colors.text.pageTitle)}>{t('search.title')}</h1>

      {/* Search mode toggle + search form */}
      <div className="flex flex-col gap-2">
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
              setClarificationApp(null)
            }}
          >
            <SearchIcon className="h-3.5 w-3.5" />
            {t('search.textSearch')}
          </button>
          <button
            type="button"
            data-testid="mode-keyword"
            className={cn(
              'flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm',
              motion.colors,
              searchMode === 'keyword'
                ? 'bg-surface text-content shadow-sm'
                : 'text-content-secondary hover:text-content',
            )}
            onClick={() => {
              setSearchMode('keyword')
              setPage(0)
              setClarificationApp(null)
            }}
          >
            <ListOrdered className="h-3.5 w-3.5" />
            {t('search.keyword')}
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
              !semanticAvailable && 'cursor-not-allowed opacity-50',
            )}
            disabled={!semanticAvailable}
            title={
              semanticAvailable
                ? undefined
                : t(
                    'search.semanticEnableHint',
                    'Semantic search is off. Turn on AI features in Settings → Advanced (takes effect after restart).',
                  )
            }
            onClick={() => {
              // #7600: defense-in-depth — `disabled` already blocks pointer/keyboard
              // activation, but guard the handler too so a programmatic click can
              // never select a mode that only returns HTTP 501 or silently-degraded
              // keyword results mislabeled as semantic.
              if (!semanticAvailable) return
              setSearchMode('semantic')
              setPage(0)
              setClarificationApp(null)
            }}
          >
            <Brain className="h-3.5 w-3.5" />
            {semanticAvailable ? t('search.semantic') : t('search.semanticUnavailable', 'Semantic (unavailable)')}
          </button>
        </div>
        {/* Scope hint: text and keyword modes search different indexes. */}
        {(searchMode === 'text' || searchMode === 'keyword') && (
          <p className="text-content-tertiary text-xs">
            {searchMode === 'keyword' ? t('search.keywordScopeHint') : t('search.textScopeHint')}
          </p>
        )}
        {/* #8059 G2c: when the embedding pipeline is not wired, the disabled
            "Semantic" toggle now carries an actionable path to turn it on
            (Settings → Advanced → Enable AI features). Does NOT change or
            auto-switch the current mode. */}
        {!semanticAvailable && (
          <p className="flex flex-wrap items-center gap-1.5 text-content-tertiary text-xs">
            <span>{t('search.semanticEnableHint')}</span>
            <button
              type="button"
              data-testid="search-enable-ai-features"
              className="text-brand-text underline underline-offset-2 hover:opacity-80"
              onClick={() => navigate('/settings/advanced')}
            >
              {t('search.enableAiFeatures')}
            </button>
          </p>
        )}
      </div>

      <form id="section-recent" onSubmit={handleSearch} className="flex gap-2">
        <Input
          data-testid="search-input"
          type="text"
          value={inputValue}
          onChange={(e) => setInputValue(e.target.value)}
          placeholder={
            searchMode === 'semantic'
              ? t('search.semanticPlaceholder')
              : searchMode === 'keyword'
                ? t('search.keywordPlaceholder')
                : t('search.placeholder')
          }
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
          <div className="space-y-3">
            <p className="text-semantic-error">{t('search.searchError')}</p>
            <div className="flex flex-wrap gap-2">
              <Button data-testid="search-error-retry" variant="secondary" size="sm" onClick={() => void refetch()}>
                {t('common.retry')}
              </Button>
              <Button data-testid="search-error-support" variant="ghost" size="sm" onClick={() => navigate('/support')}>
                {t('search.getHelp')}
              </Button>
            </div>
          </div>
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
            {t('search.results')}:{' '}
            <span className="text-brand-text">
              {activeClarificationApp ? visibleTextResults.length : response.total}
            </span>
            {t('search.resultCount')}
          </div>

          {showClarification && (
            <Card
              data-testid="history-search-clarification"
              role="region"
              aria-labelledby="history-search-clarification-title"
              padding="md"
              className="space-y-3"
            >
              <div className="space-y-1">
                <h2 id="history-search-clarification-title" className={cn(typography.h3, colors.text.primary)}>
                  {t('search.clarificationTitle')}
                </h2>
                <p className="text-content-secondary text-sm">
                  {hasCompleteTextResultSet
                    ? t('search.clarificationDescription')
                    : t('search.clarificationIncompleteDescription')}
                </p>
              </div>
              <fieldset className="flex flex-wrap gap-2">
                <legend className="sr-only">
                  {hasCompleteTextResultSet ? t('search.clarificationAppLabel') : t('search.clarificationSourceLabel')}
                </legend>
                {hasCompleteTextResultSet ? (
                  <>
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      aria-pressed={activeClarificationApp === null}
                      className={cn(activeClarificationApp === null && 'ring-1 ring-content-tertiary ring-inset')}
                      onClick={() => setClarificationApp(null)}
                    >
                      {t('search.clarificationAllApps')}
                    </Button>
                    {clarificationApps.map((app) => (
                      <Button
                        key={app}
                        type="button"
                        variant="secondary"
                        size="sm"
                        aria-pressed={activeClarificationApp === app}
                        className={cn(activeClarificationApp === app && 'ring-1 ring-content-tertiary ring-inset')}
                        onClick={() => setClarificationApp(app)}
                      >
                        {app}
                      </Button>
                    ))}
                  </>
                ) : (
                  (['frames', 'events'] as SearchType[]).map((type) => (
                    <Button
                      key={type}
                      type="button"
                      variant="secondary"
                      size="sm"
                      aria-pressed={searchType === type}
                      className={cn(searchType === type && 'ring-1 ring-content-tertiary ring-inset')}
                      onClick={() => handleTypeChange(type)}
                    >
                      {type === 'frames' ? t('search.frames') : t('search.events')}
                    </Button>
                  ))
                )}
              </fieldset>
            </Card>
          )}

          {visibleTextResults.length > 0 ? (
            <div
              id="history-search-results"
              data-testid="history-search-results"
              className="space-y-3"
              aria-live="polite"
            >
              {visibleTextResults.map((result) => (
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
            // #8059 G2c: zero text-mode results → nudge toward Keyword mode
            // (BM25 over activity summaries). The action is an explicit,
            // user-initiated switch — never a silent auto-switch.
            <EmptyState
              icon={<SearchIcon className="h-8 w-8" aria-hidden="true" />}
              title={t('search.noResults')}
              description={searchQuery ? t('search.noResultsTryKeyword') : t('search.searchHint')}
              action={
                searchQuery
                  ? {
                      label: t('search.tryKeyword'),
                      onClick: () => {
                        setSearchMode('keyword')
                        setPage(0)
                      },
                    }
                  : undefined
              }
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

      {/* Keyword (FTS/BM25) search results */}
      {searchMode === 'keyword' && keywordResults && (
        <>
          <div className="text-content-secondary">
            "<span className="text-content">{searchQuery}</span>" {t('search.results')}:{' '}
            <span className="text-brand-text">{keywordResults.length}</span>
            {t('search.resultCount')}
          </div>

          {keywordResults.length > 0 ? (
            <div className="space-y-3">
              {keywordResults.map((result, index) => (
                <KeywordResultCard key={result.segment_id} result={result} rank={index + 1} query={searchQuery} />
              ))}
            </div>
          ) : (
            <EmptyState
              icon={<ListOrdered className="h-8 w-8" aria-hidden="true" />}
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

// ── Keyword (FTS/BM25) search result card ──────────────────────
//
// Keyword results are already ordered by BM25 relevance server-side
// (`ORDER BY rank`). The raw `score` field carries the FTS5 bm25() value (a
// negative float where more-negative = more relevant), NOT a 0..1 similarity —
// so it must NOT be rendered as a percentage the way SemanticResultCard does.
// Instead we surface the honest signal: the 1-based relevance position.

interface KeywordResultCardProps {
  result: SemanticSearchResult
  rank: number
  query: string
}

function KeywordResultCard({ result, rank, query }: KeywordResultCardProps) {
  const { t } = useTranslation()
  const body = result.llm_summary || result.original_text

  return (
    <Card padding="md" className="flex gap-4">
      {/* Relevance rank ordinal */}
      <div className="flex flex-shrink-0 flex-col items-center justify-center gap-1">
        <span className={cn('text-xl', typography.weight.bold, 'text-brand-text')}>#{rank}</span>
        <span className="text-content-tertiary text-xs">{t('search.relevance')}</span>
      </div>

      {/* Content */}
      <div className="min-w-0 flex-1">
        <div className="mb-1 flex flex-wrap items-center gap-2">
          <Badge color="primary" size="sm">
            {result.dominant_category || result.content_type}
          </Badge>
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

        {body && <div className="mt-1 line-clamp-3 text-content-secondary text-sm">{highlightText(body, query)}</div>}
      </div>
    </Card>
  )
}
