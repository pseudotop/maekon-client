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

import { fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import Search from './Search'

const mockFetchTags = vi.fn().mockResolvedValue([])
const mockSearch = vi.fn().mockResolvedValue({ query: '', total: 0, results: [] })
const mockFetchSemanticSearch = vi.fn().mockResolvedValue([])
const mockFetchSemanticSearchCapabilities = vi.fn()

vi.mock('../api/client', () => ({
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
