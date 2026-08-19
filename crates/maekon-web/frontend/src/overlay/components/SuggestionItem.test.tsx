import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { SuggestionViewDto } from '../types'

// #7917 / ADR-027: the one-click Run affordance renders ONLY when the DTO
// carries a derived action binding, invokes the composition-root command with
// the suggestion id (never a preset id), and stays disabled while a run is
// in flight (the command can block up to the 30s HITL confirm timeout).

const showToastMock = vi.fn()
vi.mock('./Toast', () => ({
  showToast: (message: string, type?: string) => showToastMock(message, type),
}))

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    // Return the default value with basic {{var}} interpolation.
    t: (_key: string, defaultValue?: string | Record<string, unknown>, opts?: Record<string, unknown>) => {
      if (typeof defaultValue !== 'string') return _key
      const vars = opts ?? {}
      return defaultValue.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(vars[name] ?? `{{${name}}}`))
    },
    i18n: { language: 'en' },
  }),
}))

const invokeMock = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

import { SuggestionItem } from './SuggestionItem'

function makeSuggestion(overrides: Partial<SuggestionViewDto> = {}): SuggestionViewDto {
  return {
    id: 'sug-1',
    title: 'Need focus time',
    body: 'Communication apps dominated the last hour.',
    priority: 'high',
    category: 'focus',
    source: 'rule_based',
    confidence_score: 0.9,
    created_at: '2026-07-08T10:30:00Z',
    reasoning: null,
    ...overrides,
  } as SuggestionViewDto
}

describe('SuggestionItem action binding (#7917 / ADR-027)', () => {
  beforeEach(() => {
    showToastMock.mockClear()
    invokeMock.mockReset()
  })

  it('renders no Run button and keeps the lock badge when the DTO carries no action', () => {
    render(<SuggestionItem item={makeSuggestion()} onAction={vi.fn()} />)

    expect(screen.queryByTestId('run-suggestion-action')).toBeNull()
    expect(screen.getByText('No auto action')).toBeInTheDocument()
  })

  it('renders the Run button with the preset label and the policy-neutral gate copy when bound', () => {
    render(<SuggestionItem item={makeSuggestion({ action: { label: 'Deep Work Start' } })} onAction={vi.fn()} />)

    const button = screen.getByTestId('run-suggestion-action')
    expect(button).toHaveTextContent('Run Deep Work Start')
    // Copy must not promise a confirmation prompt (field default is Auto).
    expect(screen.getByText('Runs through the automation gate — your confirmation settings apply.')).toBeInTheDocument()
    expect(screen.queryByText('No auto action')).toBeNull()
  })

  it('keeps a single highlighted action and lays review actions out in a readable two-column grid', () => {
    render(<SuggestionItem item={makeSuggestion({ action: { label: 'Deep Work Start' } })} onAction={vi.fn()} />)

    expect(screen.getByTestId('run-suggestion-action')).toHaveClass('w-full', 'bg-brand')
    expect(screen.getByTestId('suggestion-review-actions')).toHaveClass('grid', 'grid-cols-2')

    for (const label of ['suggestions.accept', 'suggestions.reject', 'suggestions.later', 'suggestions.explain']) {
      const button = screen.getByText(label).closest('button')
      expect(button).toHaveClass('whitespace-nowrap', 'bg-surface-muted')
      expect(button?.className).not.toMatch(/semantic-(success|error)/)
    }
  })

  it('invokes run_suggestion_action with the suggestion id and reports success', async () => {
    invokeMock.mockResolvedValueOnce(undefined)
    const onRan = vi.fn()
    render(
      <SuggestionItem
        item={makeSuggestion({ action: { label: 'Deep Work Start' } })}
        onAction={vi.fn()}
        onRan={onRan}
      />,
    )

    fireEvent.click(screen.getByTestId('run-suggestion-action'))

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('run_suggestion_action', { suggestionId: 'sug-1' })
    })
    await waitFor(() => {
      expect(showToastMock).toHaveBeenCalledWith('Action ran', 'success')
    })
    expect(onRan).toHaveBeenCalledTimes(1)
  })

  it('disables the button while the run is in flight and re-enables afterwards', async () => {
    let resolveRun: (() => void) | undefined
    invokeMock.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveRun = resolve
        }),
    )
    render(<SuggestionItem item={makeSuggestion({ action: { label: 'Deep Work Start' } })} onAction={vi.fn()} />)

    const button = screen.getByTestId('run-suggestion-action')
    fireEvent.click(button)

    await waitFor(() => {
      expect(button).toBeDisabled()
      expect(button).toHaveTextContent('Running…')
    })
    // A second click while pending must not start another run.
    fireEvent.click(button)
    expect(invokeMock).toHaveBeenCalledTimes(1)

    resolveRun?.()
    await waitFor(() => {
      expect(button).not.toBeDisabled()
    })
  })

  it('shows the failure toast and re-enables the button when the run is refused', async () => {
    invokeMock.mockRejectedValueOnce({ code: 'automation.disabled', message: 'automation disabled' })
    render(<SuggestionItem item={makeSuggestion({ action: { label: 'Deep Work Start' } })} onAction={vi.fn()} />)

    const button = screen.getByTestId('run-suggestion-action')
    fireEvent.click(button)

    await waitFor(() => {
      expect(showToastMock).toHaveBeenCalledWith(expect.stringContaining('Could not run action:'), 'error')
    })
    expect(button).not.toBeDisabled()
  })
})
