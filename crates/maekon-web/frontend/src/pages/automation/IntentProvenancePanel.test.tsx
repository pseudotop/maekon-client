import { screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import {
  makeAttempt,
  makeFallback,
  makeProvenance,
  PRIVATE_CANARY,
  SUBMISSION_A,
} from '../../api/__tests__/fixtures/intentExecution'
import { decodeIntentProvenance } from '../../api/intentExecution'
import i18n from '../../i18n'
import en from '../../i18n/locales/en.json'
import es from '../../i18n/locales/es.json'
import ja from '../../i18n/locales/ja.json'
import ko from '../../i18n/locales/ko.json'
import zh from '../../i18n/locales/zh-CN.json'
import { IntentProvenancePanel } from './IntentProvenancePanel'

afterEach(async () => {
  await i18n.changeLanguage('en')
})
const panel = () => screen.getByTestId('intent-provenance')

describe('response-bound intent presentation', () => {
  it('shows actual primary planning and keeps unobserved skill use unknown', () => {
    renderWithProviders(<IntentProvenancePanel provenance={makeProvenance()} />)
    expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('Model-based plan')
    expect(panel()).toHaveTextContent('Skill use was not observed')
    expect(panel()).toHaveTextContent('Complete observation')
    expect(panel()).toHaveTextContent('Completed')
    expect(panel()).not.toHaveTextContent('Skills were used')
  })

  it('shows both observed fallback legs and the rule skill limitation', () => {
    renderWithProviders(<IntentProvenancePanel provenance={makeFallback()} />)
    expect(screen.getAllByTestId('intent-provenance-attempt')).toHaveLength(2)
    expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('Rule-based plan')
    expect(panel()).toHaveTextContent('Provider unavailable')
    expect(panel()).toHaveTextContent('This route does not support skills')
    expect(screen.getAllByTestId('intent-observed-event').map((e) => e.dataset.sequence)).toEqual([
      '0',
      '1',
      '2',
      '3',
      '4',
      '5',
      '6',
      '7',
    ])
  })

  it('does not invent a failed model attempt for configuration-only rule degradation', () => {
    const rule = makeFallback().attempts[1]
    renderWithProviders(
      <IntentProvenancePanel
        provenance={makeProvenance({
          configuration_reason: 'rule_only_degradation',
          planning_outcome: 'rule',
          attempts: [{ ...rule, predecessor: null }],
        })}
      />,
    )
    expect(screen.getAllByTestId('intent-provenance-attempt')).toHaveLength(1)
    expect(screen.getByTestId('intent-configuration-degradation')).toHaveTextContent(
      'No failed model attempt was observed',
    )
    expect(screen.getByTestId('intent-provenance-attempt')).toHaveTextContent('Rules — Succeeded')
    expect(screen.getByTestId('intent-provenance-attempt')).not.toHaveTextContent('Remote model')
  })

  it('renders both-leg failure and observed denial without claiming action completion', () => {
    const attempts = makeFallback().attempts
    attempts[1] = { ...attempts[1], outcome: 'error', reason: 'unsupported_action' }
    const { rerender } = renderWithProviders(
      <IntentProvenancePanel
        provenance={makeProvenance({ attempts, planning_outcome: 'error', action_outcome: 'not_started' })}
      />,
    )
    expect(panel()).toHaveTextContent('Planning failed')
    expect(panel()).toHaveTextContent('Action is unsupported')
    expect(within(panel()).queryByText('Completed')).not.toBeInTheDocument()
    rerender(
      <IntentProvenancePanel
        provenance={makeProvenance({
          attempts: [],
          planning_outcome: 'denied',
          action_outcome: 'not_started',
          observation_reason: 'denied',
        })}
      />,
    )
    expect(panel()).toHaveTextContent('Planning denied')
    expect(panel()).toHaveTextContent('Not started')
    expect(screen.queryAllByTestId('intent-provenance-attempt')).toHaveLength(0)
  })

  it('keeps missing/legacy and partial observations uncertain', () => {
    const { rerender } = renderWithProviders(<IntentProvenancePanel provenance={null} />)
    expect(screen.getByTestId('intent-provenance-unknown')).toBeInTheDocument()
    rerender(<IntentProvenancePanel provenance={makeProvenance({ observation: 'legacy_unknown' })} />)
    expect(screen.getByTestId('intent-provenance-unknown')).toBeInTheDocument()
    expect(screen.queryByTestId('intent-observed-plan')).not.toBeInTheDocument()
    rerender(
      <IntentProvenancePanel provenance={makeProvenance({ observation: 'partial', action_outcome: 'not_started' })} />,
    )
    expect(panel()).toHaveTextContent('Partial observation')
    expect(within(panel()).queryByText('Not started')).not.toBeInTheDocument()
    expect(panel()).not.toHaveTextContent('Cancelled')
  })

  it('renders only supported, localized reason and capability labels', () => {
    const p = makeProvenance({
      attempts: [makeAttempt({ outcome: 'timeout', reason: 'timeout' })],
      planning_outcome: 'error',
      action_outcome: 'not_started',
    })
    const { rerender } = renderWithProviders(<IntentProvenancePanel provenance={p} />)
    expect(panel()).toHaveTextContent('Provider timed out')
    const unsafe = { ...p, endpoint: PRIVATE_CANARY }
    rerender(<IntentProvenancePanel provenance={decodeIntentProvenance(unsafe, SUBMISSION_A)} />)
    expect(panel()).not.toHaveTextContent(PRIVATE_CANARY)
    expect(screen.getByTestId('intent-provenance-unknown')).toBeInTheDocument()
  })

  it('uses real Korean resources and keeps all five locale keys and interpolation tokens in parity', async () => {
    await i18n.changeLanguage('ko')
    renderWithProviders(<IntentProvenancePanel provenance={makeFallback()} />)
    expect(screen.getByTestId('intent-observed-plan')).toHaveTextContent('규칙 기반 계획')
    expect(panel()).toHaveTextContent('이 처리 방식은 스킬을 지원하지 않음')
    const flatten = (value: Record<string, unknown>, prefix = ''): Record<string, string> =>
      Object.fromEntries(
        Object.entries(value).flatMap(([key, item]) =>
          typeof item === 'string'
            ? [[prefix + key, item]]
            : Object.entries(flatten(item as Record<string, unknown>, `${prefix}${key}.`)),
        ),
      )
    const baseline = flatten(en.automation.intentProvenance)
    for (const locale of [ko, es, ja, zh]) {
      const translated = flatten(locale.automation.intentProvenance)
      expect(Object.keys(translated).sort()).toEqual(Object.keys(baseline).sort())
      for (const [key, text] of Object.entries(translated)) {
        expect(text.trim()).not.toBe('')
        expect(text.match(/\{\{[^}]+\}\}/g) ?? []).toEqual(baseline[key].match(/\{\{[^}]+\}\}/g) ?? [])
      }
    }
  })
})
