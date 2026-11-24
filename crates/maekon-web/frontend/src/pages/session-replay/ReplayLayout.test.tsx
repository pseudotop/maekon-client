import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { createInstance } from 'i18next'
import { I18nextProvider } from 'react-i18next'
import { MemoryRouter, Route, Routes, useOutletContext } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import en from '../../i18n/locales/en.json'
import es from '../../i18n/locales/es.json'
import ja from '../../i18n/locales/ja.json'
import ko from '../../i18n/locales/ko.json'
import zhCN from '../../i18n/locales/zh-CN.json'

const policyPrimary: Record<string, string> = {
  en: 'Policy denied',
  ko: '정책에 의해 거부되었습니다',
  ja: 'ポリシーにより拒否されました',
  'zh-CN': '被策略拒绝',
  es: 'Denegado por la política',
}
const resources = {
  en: { translation: en },
  ko: { translation: ko },
  ja: { translation: ja },
  'zh-CN': { translation: zhCN },
  es: { translation: es },
}
async function testLocale(language = 'en') {
  const locale = createInstance()
  await locale.init({ lng: language, fallbackLng: 'en', resources, interpolation: { escapeValue: false } })
  return locale
}

import ReplayLayout, { type ReplayOutletContext } from './ReplayLayout'

const api = vi.hoisted(() => ({
  timeline: vi.fn(),
  settings: vi.fn().mockResolvedValue({
    ai_provider: { scene_intelligence: { enabled: false, overlay_enabled: false, allow_action_execution: false } },
  }),
  scene: vi.fn(),
  calibration: vi.fn(),
}))
vi.mock('../../api/client', () => ({
  fetchTimeline: api.timeline,
  fetchSettings: api.settings,
  fetchAutomationScene: api.scene,
  fetchSceneCalibration: api.calibration,
}))
vi.mock('./SceneOverlay', () => ({ useSceneState: () => ({}), SceneAssistantPanel: () => null }))
vi.mock('./SessionPlayback', () => ({ TimelineScrubberSection: () => null }))
function Child() {
  const context = useOutletContext<ReplayOutletContext>()
  return (
    <output data-testid="replay-child">
      {JSON.stringify({
        timeline: context.timeline,
        enabled: context.sceneIntelligenceEnabled,
        overlay: context.overlayAllowed,
        execute: context.sceneExecutionAllowed,
      })}
    </output>
  )
}
const emptyTimeline = {
  session: { start: '2026-11-11T00:00:00Z', end: '2026-11-11T01:00:00Z' },
  items: [],
  segments: [],
}
const clients: QueryClient[] = []
async function mount(language = 'en') {
  const locale = await testLocale(language)
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  clients.push(client)
  render(
    <I18nextProvider i18n={locale}>
      <QueryClientProvider client={client}>
        <MemoryRouter initialEntries={['/replay']}>
          <Routes>
            <Route path="/replay" element={<ReplayLayout />}>
              <Route index element={<Child />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </I18nextProvider>,
  )
  return { locale, client }
}
describe('ReplayLayout actual query failures', () => {
  afterEach(() => {
    clients.forEach((client) => {
      client.clear()
    })
    clients.length = 0
    vi.restoreAllMocks()
  })
  it.each(
    Object.keys(resources).flatMap((language) =>
      ['future.timeline', 'policy.denied'].map((code) => ({ language, code })),
    ),
  )('localizes timeline failure and copies collapsed detail in %s', async ({ language, code }) => {
    api.timeline.mockReset().mockRejectedValue({ code, message: 'timeline diagnostic' })
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const { locale } = await mount(language)
    const alert = await screen.findByRole('alert')
    expect(alert.querySelector('p')).toHaveTextContent(
      code === 'policy.denied' ? policyPrimary[language] : locale.t('errors.generic'),
    )
    expect(alert.querySelector('p')).not.toHaveTextContent('timeline diagnostic')
    const detail = within(alert).getByTestId('support-details')
    expect(detail).not.toHaveAttribute('open')
    fireEvent.click(detail.querySelector('summary')!)
    fireEvent.click(within(detail).getByRole('button'))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(`${code}: timeline diagnostic`))
    expect(screen.getByTestId('replay-child')).toHaveTextContent('"timeline":null')
    expect(api.timeline.mock.calls[0][0]).toEqual({
      from: expect.stringMatching(/Z$/),
      to: expect.stringMatching(/Z$/),
    })
    expect(api.scene).not.toHaveBeenCalled()
    expect(api.calibration).not.toHaveBeenCalled()
  })
  it('shows a generic error for null rejection, then recovers the same QueryClient and Outlet', async () => {
    api.timeline.mockReset().mockRejectedValue(null)
    const { locale, client } = await mount('ja')
    await waitFor(() =>
      expect(client.getQueryCache().find({ queryKey: ['timeline'], exact: false, type: 'active' })?.state.status).toBe(
        'error',
      ),
    )
    expect(await screen.findByRole('alert')).toHaveTextContent(locale.t('errors.generic'))
    expect(screen.queryByTestId('support-details')).not.toBeInTheDocument()
    expect(screen.getByTestId('replay-child')).toHaveTextContent('"timeline":null')
    const activeQuery = client.getQueryCache().find({ queryKey: ['timeline'], exact: false, type: 'active' })!
    const callsBeforeRecovery = api.timeline.mock.calls.length
    api.timeline.mockResolvedValue(emptyTimeline)
    await act(async () => {
      await client.refetchQueries({ queryKey: activeQuery.queryKey, exact: true })
    })
    await waitFor(() =>
      expect(client.getQueryCache().find({ queryKey: ['timeline'], exact: false, type: 'active' })?.state.status).toBe(
        'success',
      ),
    )
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.getByTestId('replay-child')).toHaveTextContent('"items":[]')
    expect(screen.getByTestId('replay-child')).toHaveTextContent('"execute":false')
    expect(api.timeline).toHaveBeenCalledTimes(callsBeforeRecovery + 1)
    expect(activeQuery.state.status).toBe('success')
  })
})
