/**
 * Search page — semantic-mode honesty guard (#7600 / #7643 / #7668).
 *
 * Covers the truth-in-UI contract that the shipped release build (embedding
 * feature compiled out, no vector/embedding ports wired into the web
 * AppState) must not present "Semantic" as a working search mode:
 *
 *   - `semantic_available: false` (the shipped-build default) disables the
 *     "Semantic" toggle (both the `disabled` attribute and the click
 *     handler's defense-in-depth early-return), and no request is ever
 *     issued with `mode=semantic`.
 *   - `semantic_available: true` (a build/config where the vector pipeline
 *     IS wired) enables the toggle and a search explicitly requests
 *     `mode=semantic` — never the silently-degrading default `mode=hybrid`.
 */

import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import en from '../i18n/locales/en.json'
import Search from './Search'

const mockFetchTags = vi.fn().mockResolvedValue([])
const mockSearch = vi.fn().mockResolvedValue({ query: '', total: 0, results: [] })
const mockFetchSemanticSearch = vi.fn().mockResolvedValue([])
const mockFetchSemanticSearchCapabilities = vi.fn()

// Spread the real module: a bare factory drops exports the subject
// imports but this test does not stub (e.g. TAGS_QUERY_KEY).
vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  fetchTags: (...args: unknown[]) => mockFetchTags(...args),
  search: (...args: unknown[]) => mockSearch(...args),
  fetchSemanticSearch: (...args: unknown[]) => mockFetchSemanticSearch(...args),
  fetchSemanticSearchCapabilities: (...args: unknown[]) => mockFetchSemanticSearchCapabilities(...args),
}))

describe('Search page — semantic mode honesty', () => {
  beforeEach(() => {
    mockFetchTags.mockClear().mockResolvedValue([])
    mockSearch.mockClear().mockResolvedValue({ query: '', total: 0, results: [] })
    mockFetchSemanticSearch.mockClear().mockResolvedValue([])
    mockFetchSemanticSearchCapabilities.mockClear()
  })

  it('disables the Semantic toggle and never issues mode=semantic when the build/config has no vector pipeline wired', async () => {
    mockFetchSemanticSearchCapabilities.mockResolvedValue({ semantic_available: false })

    renderWithProviders(<Search />)

    const semanticToggle = await screen.findByTestId('mode-semantic')
    await waitFor(() => expect(semanticToggle).toBeDisabled())

    // Defense-in-depth: even a programmatic click (bypassing the `disabled`
    // attribute) must not flip the mode — the onClick handler re-checks
    // `semanticAvailable` before calling setSearchMode.
    fireEvent.click(semanticToggle)

    const input = screen.getByTestId('search-input')
    fireEvent.change(input, { target: { value: 'auth module' } })
    fireEvent.submit(input.closest('form') as HTMLFormElement)

    await waitFor(() => expect(mockSearch).toHaveBeenCalled())
    expect(mockFetchSemanticSearch).not.toHaveBeenCalled()
    expect(semanticToggle).toBeDisabled()
  })

  it('enables the Semantic toggle and requests mode=semantic (never the silently-degrading hybrid default) when the pipeline is wired', async () => {
    mockFetchSemanticSearchCapabilities.mockResolvedValue({ semantic_available: true })

    renderWithProviders(<Search />)

    const semanticToggle = await screen.findByTestId('mode-semantic')
    await waitFor(() => expect(semanticToggle).not.toBeDisabled())

    fireEvent.click(semanticToggle)

    const input = screen.getByTestId('search-input')
    fireEvent.change(input, { target: { value: 'auth module' } })
    fireEvent.submit(input.closest('form') as HTMLFormElement)

    await waitFor(() => expect(mockFetchSemanticSearch).toHaveBeenCalledWith('auth module', 20, 'semantic'))
    expect(mockSearch).not.toHaveBeenCalled()
  })
})

/**
 * Keyword (FTS/BM25) mode — #7912 T3.3-Tier1.
 *
 * The new keyword/relevance mode hits the storage `text_search` FTS path
 * (SQLite FTS5 CJK-bigram + BM25) via `mode=keyword` over enriched activity
 * segments — a distinct scope from the untouched LIKE text mode (frame/event).
 * Unlike "semantic", keyword is embeddings-free and therefore always available,
 * so it must NOT be gated on the semantic-capability check.
 */
describe('Search page — keyword (FTS/BM25) mode', () => {
  beforeEach(() => {
    mockFetchTags.mockClear().mockResolvedValue([])
    mockSearch.mockClear().mockResolvedValue({ query: '', total: 0, results: [] })
    mockFetchSemanticSearch.mockClear().mockResolvedValue([])
    mockFetchSemanticSearchCapabilities.mockClear().mockResolvedValue({ semantic_available: false })
  })

  it('runs an FTS keyword search (mode=keyword) — always enabled even when the semantic pipeline is absent', async () => {
    renderWithProviders(<Search />)

    const keywordToggle = await screen.findByTestId('mode-keyword')
    // Keyword mode is embeddings-free, so it stays enabled regardless of the
    // semantic-capability result (which is `false` here).
    expect(keywordToggle).not.toBeDisabled()
    fireEvent.click(keywordToggle)

    const input = screen.getByTestId('search-input')
    fireEvent.change(input, { target: { value: 'authentication' } })
    fireEvent.submit(input.closest('form') as HTMLFormElement)

    await waitFor(() => expect(mockFetchSemanticSearch).toHaveBeenCalledWith('authentication', 20, 'keyword'))
    // The untouched LIKE text-mode endpoint must NOT be hit in keyword mode.
    expect(mockSearch).not.toHaveBeenCalled()
  })

  it('renders keyword results with an ordinal relevance rank (BM25 order preserved server-side)', async () => {
    mockFetchSemanticSearch.mockResolvedValue([
      {
        segment_id: 'seg-1',
        content_type: 'segment',
        content_label: 'Deep work',
        original_text: 'authentication module refactor',
        // Raw FTS5 bm25() rank (negative float), NOT a 0..1 score.
        score: -3.2,
        similarity: 0,
        time_decay: 0,
        timestamp: '2026-03-18T10:00:00Z',
        segment_start: null,
        segment_end: null,
        duration_secs: 1800,
        llm_summary: 'Refactored the authentication module',
        dominant_category: 'Development',
        regime_label: null,
      },
    ])

    renderWithProviders(<Search />)

    fireEvent.click(await screen.findByTestId('mode-keyword'))
    const input = screen.getByTestId('search-input')
    fireEvent.change(input, { target: { value: 'authentication' } })
    fireEvent.submit(input.closest('form') as HTMLFormElement)

    await waitFor(() => expect(mockFetchSemanticSearch).toHaveBeenCalledWith('authentication', 20, 'keyword'))
    // Ordinal relevance position, not a bogus "-320%" from the raw BM25 score.
    expect(await screen.findByText('#1')).toBeInTheDocument()
    expect(screen.getByText('Development')).toBeInTheDocument()
    expect(screen.getByText('Deep work')).toBeInTheDocument()
  })
})

/**
 * #8059 G2c — discoverability nudges.
 *
 * When the semantic pipeline is not wired, the disabled "Semantic" toggle now
 * carries an actionable path to turn AI features on (Settings → Advanced), and
 * a zero-result text search suggests trying Keyword mode. Neither nudge changes
 * or silently auto-switches the current mode.
 */
describe('Search page — G2c discoverability nudges', () => {
  beforeEach(() => {
    mockFetchTags.mockClear().mockResolvedValue([])
    mockSearch.mockClear().mockResolvedValue({ query: '', total: 0, results: [] })
    mockFetchSemanticSearch.mockClear().mockResolvedValue([])
    mockFetchSemanticSearchCapabilities.mockClear()
  })

  it('shows an actionable "Enable AI features" hint when the semantic pipeline is unavailable', async () => {
    mockFetchSemanticSearchCapabilities.mockResolvedValue({ semantic_available: false })

    renderWithProviders(<Search />)

    const enableLink = await screen.findByTestId('search-enable-ai-features')
    expect(enableLink).toHaveTextContent(en.search.enableAiFeatures)
  })

  it('hides the AI-features hint once semantic search is available', async () => {
    mockFetchSemanticSearchCapabilities.mockResolvedValue({ semantic_available: true })

    renderWithProviders(<Search />)

    // Wait for the toggle to become enabled (capability resolved), then assert
    // the nudge is gone.
    const semanticToggle = await screen.findByTestId('mode-semantic')
    await waitFor(() => expect(semanticToggle).not.toBeDisabled())
    expect(screen.queryByTestId('search-enable-ai-features')).not.toBeInTheDocument()
  })

  it('offers a Keyword-mode suggestion on a zero-result text search and switches mode only on explicit click', async () => {
    mockFetchSemanticSearchCapabilities.mockResolvedValue({ semantic_available: false })
    mockSearch.mockResolvedValue({ query: 'auth', total: 0, offset: 0, limit: 20, results: [] })

    renderWithProviders(<Search />)

    const input = await screen.findByTestId('search-input')
    fireEvent.change(input, { target: { value: 'auth' } })
    fireEvent.submit(input.closest('form') as HTMLFormElement)

    // Zero results → the empty state surfaces the Keyword suggestion action.
    const tryKeyword = await screen.findByRole('button', { name: en.search.tryKeyword })
    // No silent auto-switch: keyword search must not have run before the click.
    expect(mockFetchSemanticSearch).not.toHaveBeenCalled()

    fireEvent.click(tryKeyword)

    await waitFor(() => expect(mockFetchSemanticSearch).toHaveBeenCalledWith('auth', 20, 'keyword'))
  })
})

/**
 * Error-state recovery — #8079 (CRT-PRV-QC-CJ-00-09 / CRT-PRV-UX-RECOVERY-001).
 *
 * A failed search must offer an actionable path (retry the active query + a
 * link to the Support & Diagnostics destination) instead of a dead-end error
 * message.
 */
describe('Search page — error-state recovery', () => {
  beforeEach(() => {
    mockFetchTags.mockClear().mockResolvedValue([])
    mockSearch.mockClear()
    mockFetchSemanticSearch.mockClear().mockResolvedValue([])
    mockFetchSemanticSearchCapabilities.mockClear().mockResolvedValue({ semantic_available: false })
  })

  it('surfaces a retry and a support link when the search request fails, and retry re-issues the query', async () => {
    mockSearch.mockRejectedValue(new Error('boom'))

    renderWithProviders(<Search />, { routerProps: { initialEntries: ['/search?q=auth'] } })

    // The error card renders both recovery affordances.
    const retry = await screen.findByTestId('search-error-retry')
    expect(retry).toBeInTheDocument()
    expect(screen.getByTestId('search-error-support')).toBeInTheDocument()

    const callsBeforeRetry = mockSearch.mock.calls.length
    fireEvent.click(retry)

    await waitFor(() => expect(mockSearch.mock.calls.length).toBeGreaterThan(callsBeforeRetry))
  })
})

describe('Search page — trustworthy history clarification', () => {
  beforeEach(() => {
    mockFetchTags.mockClear().mockResolvedValue([])
    mockSearch.mockClear()
    mockFetchSemanticSearch.mockClear().mockResolvedValue([])
    mockFetchSemanticSearchCapabilities.mockClear().mockResolvedValue({ semantic_available: false })
  })

  it('asks the user to clarify a complete ambiguous result set and narrows it only on explicit app choice', async () => {
    mockSearch.mockResolvedValue({
      query: 'Delta',
      total: 4,
      offset: 0,
      limit: 20,
      results: [
        {
          result_type: 'frame',
          id: '1',
          timestamp: '2026-07-20T08:49:00Z',
          app_name: 'Maekon QC Notes',
          window_title: 'Project Delta launch checklist',
          matched_text: 'Project Delta review completed.',
          image_url: null,
          importance: 0.92,
          tags: [],
        },
        {
          result_type: 'frame',
          id: '2',
          timestamp: '2026-07-20T08:45:00Z',
          app_name: 'Maekon QC Browser',
          window_title: 'Delta research - local fixture',
          matched_text: 'Synthetic local research context.',
          image_url: null,
          importance: 0.84,
          tags: [],
        },
        {
          result_type: 'frame',
          id: '3',
          timestamp: '2026-07-20T08:40:00Z',
          app_name: 'Maekon QC Mail',
          window_title: 'Project Delta follow-up for [EMAIL]',
          matched_text: 'Synthetic contact [EMAIL] and card [CARD] must be redacted.',
          image_url: null,
          importance: 0.78,
          tags: [],
        },
        {
          result_type: 'event',
          id: '4',
          timestamp: '2026-07-20T08:35:00Z',
          app_name: 'Maekon QC Editor',
          window_title: 'Delta annotation workspace',
          matched_text: 'A fourth distinct context.',
          image_url: null,
          importance: null,
        },
      ],
    })

    renderWithProviders(<Search />, { routerProps: { initialEntries: ['/search?q=Delta'] } })

    expect(await screen.findByTestId('history-search-clarification')).toBeInTheDocument()
    expect(screen.getByText(en.search.clarificationTitle)).toBeInTheDocument()
    expect(screen.getByText(en.search.clarificationDescription)).toBeInTheDocument()
    const resultList = screen.getByTestId('history-search-results')
    expect(resultList).toHaveTextContent('Delta research - local fixture')
    expect(resultList).toHaveTextContent('Project Delta launch checklist')

    fireEvent.click(screen.getByRole('button', { name: 'Maekon QC Browser' }))

    expect(resultList).toHaveTextContent('Delta research - local fixture')
    expect(resultList).not.toHaveTextContent('Project Delta launch checklist')
    expect(screen.getByRole('button', { name: 'Maekon QC Browser' })).toHaveAttribute('aria-pressed', 'true')

    fireEvent.click(screen.getByRole('button', { name: en.search.clarificationAllApps }))
    expect(resultList).toHaveTextContent('Project Delta launch checklist')
  })

  it('does not interrupt an exact query that resolves to fewer than three distinct contexts', async () => {
    mockSearch.mockResolvedValue({
      query: 'glacier-orbit',
      total: 3,
      offset: 0,
      limit: 20,
      results: [
        {
          result_type: 'frame',
          id: '1',
          timestamp: '2026-07-20T08:45:00Z',
          app_name: 'Maekon QC Browser',
          window_title: 'Delta research - local fixture',
          matched_text: 'Keyword target: glacier-orbit.',
          image_url: null,
          importance: 0.84,
          tags: [],
        },
        {
          result_type: 'frame',
          id: '2',
          timestamp: '2026-07-20T08:27:00Z',
          app_name: 'Maekon QC Terminal',
          window_title: 'glacier-orbit keyword trace',
          matched_text: 'glacier-orbit exact keyword result.',
          image_url: null,
          importance: 0.66,
          tags: [],
        },
        {
          result_type: 'event',
          id: '3',
          timestamp: '2026-07-20T08:27:00Z',
          app_name: 'Maekon QC Terminal',
          window_title: 'glacier-orbit keyword trace',
          matched_text: 'glacier-orbit exact keyword result.',
          image_url: null,
          importance: null,
        },
      ],
    })

    renderWithProviders(<Search />, { routerProps: { initialEntries: ['/search?q=glacier-orbit'] } })

    expect(await screen.findByTestId('history-search-results')).toHaveTextContent('Delta research - local fixture')
    expect(screen.queryByTestId('history-search-clarification')).not.toBeInTheDocument()
  })

  it('keeps the clarification prompt for paged results and offers server-side source refinements', async () => {
    mockSearch.mockResolvedValue({
      query: 'Delta',
      total: 25,
      offset: 0,
      limit: 20,
      results: [
        {
          result_type: 'frame',
          id: '1',
          timestamp: '2026-07-20T08:49:00Z',
          app_name: 'Maekon QC Notes',
          window_title: 'Project Delta launch checklist',
          matched_text: 'First context.',
          image_url: null,
          importance: 0.92,
          tags: [],
        },
        {
          result_type: 'frame',
          id: '2',
          timestamp: '2026-07-20T08:45:00Z',
          app_name: 'Maekon QC Browser',
          window_title: 'Delta research workspace',
          matched_text: 'Second context.',
          image_url: null,
          importance: 0.84,
          tags: [],
        },
        {
          result_type: 'event',
          id: '3',
          timestamp: '2026-07-20T08:40:00Z',
          app_name: 'Maekon QC Mail',
          window_title: 'Project Delta follow-up',
          matched_text: 'Third context.',
          image_url: null,
          importance: null,
        },
      ],
    })

    renderWithProviders(<Search />, { routerProps: { initialEntries: ['/search?q=Delta'] } })

    const clarification = await screen.findByTestId('history-search-clarification')
    expect(clarification).toBeInTheDocument()
    expect(screen.getByText(en.search.clarificationIncompleteDescription)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Maekon QC Browser' })).not.toBeInTheDocument()

    fireEvent.click(within(clarification).getByRole('button', { name: en.search.frames }))

    await waitFor(() => expect(mockSearch).toHaveBeenLastCalledWith(expect.objectContaining({ searchType: 'frames' })))
  })
})
