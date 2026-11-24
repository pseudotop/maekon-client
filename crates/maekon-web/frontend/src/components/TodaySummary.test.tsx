import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import i18n from '../i18n'
import TodaySummary from './TodaySummary'

describe('TodaySummary', () => {
  it('renders nothing for a day without a summary', () => {
    const { container } = renderWithProviders(<TodaySummary headline={{ kind: 'none' }} />)
    expect(container).toBeEmptyDOMElement()
  })

  it('holds the slot with a skeleton, and claims no provenance, while loading', () => {
    renderWithProviders(<TodaySummary headline={{ kind: 'none' }} loading />)
    expect(screen.getByTestId('today-summary')).toHaveAttribute('aria-busy', 'true')
    expect(screen.queryByTestId('today-summary-headline')).not.toBeInTheDocument()
    expect(screen.queryByTestId('today-summary-provenance')).not.toBeInTheDocument()
  })

  it('labels a heuristic headline as the rule-based digest', () => {
    renderWithProviders(
      <TodaySummary headline={{ kind: 'heuristic', variant: 'app', topApp: 'Figma', longestFocusSecs: 0 }} />,
    )
    expect(screen.getByTestId('today-summary-headline')).toHaveTextContent(
      i18n.t('dashboard.todaySummary.headline.app', { app: 'Figma' }),
    )
    expect(screen.getByTestId('today-summary-provenance')).toHaveTextContent(
      i18n.t('summaryProvenance.digest.heuristic'),
    )
  })

  it('labels an AI headline with its provider class', () => {
    renderWithProviders(
      <TodaySummary
        headline={{ kind: 'ai', text: 'Wrote the release notes.', providerClass: 'external_api', generatedAt: null }}
      />,
    )
    expect(screen.getByTestId('today-summary-headline')).toHaveTextContent('Wrote the release notes.')
    const provenance = screen.getByTestId('today-summary-provenance')
    expect(provenance).toHaveAttribute('data-provenance', 'ai')
    expect(provenance).toHaveTextContent(i18n.t('summaryProvenance.provider.external_api'))
  })
})
