import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { createInstance } from 'i18next'
import { I18nextProvider } from 'react-i18next'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import en from '../i18n/locales/en.json'
import es from '../i18n/locales/es.json'
import ja from '../i18n/locales/ja.json'
import ko from '../i18n/locales/ko.json'
import zhCN from '../i18n/locales/zh-CN.json'

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

import { clearToasts, useToast } from '../hooks/useToast'
import PomodoroTimer from './PomodoroTimer'
import { ToastContainer } from './ui/Toast'

const api = vi.hoisted(() => ({ load: vi.fn(), start: vi.fn(), cancel: vi.fn(), complete: vi.fn() }))
vi.mock('../api/client', () => ({
  fetchCurrentPomodoro: api.load,
  startPomodoro: api.start,
  cancelPomodoro: api.cancel,
  completePomodoro: api.complete,
  fetchDesktopPermissionStatus: vi.fn(),
  requestDesktopNotificationPermission: vi.fn(),
}))
vi.mock('../utils/platform', () => ({
  IS_TAURI: false,
  IS_MAC: false,
  IS_WINDOWS: false,
  IS_LINUX: true,
  isTauriRuntime: () => false,
}))
function ToastProbe() {
  const { toasts } = useToast()
  return <output data-testid="toast-records">{JSON.stringify(toasts)}</output>
}
const failure = new Error('internal timer diagnostic')
function session(status = 'running', minutes = 25, rest = 5) {
  return { id: 'timer', started_at: new Date().toISOString(), status, duration_minutes: minutes, break_minutes: rest }
}
async function mount(language = 'en') {
  const locale = await testLocale(language)
  render(
    <I18nextProvider i18n={locale}>
      <PomodoroTimer />
      <ToastContainer />
      <ToastProbe />
    </I18nextProvider>,
  )
  return locale
}
function records() {
  return JSON.parse(screen.getByTestId('toast-records').textContent!)
}
async function assertDisplay(key: string, locale: Awaited<ReturnType<typeof testLocale>>, severity = 'error') {
  await waitFor(() =>
    expect(records()).toEqual([
      expect.objectContaining({ type: severity, message: locale.t(key), duration: 5000, detail: failure.message }),
    ]),
  )
  const details = screen.getAllByTestId('support-details')
  expect(details).toHaveLength(2)
  for (const detail of details) expect(detail).not.toHaveAttribute('open')
  expect(screen.getByRole('alert').querySelector('p')).toHaveTextContent(locale.t(key))
  expect(screen.getByRole('alert').querySelector('p')).not.toHaveTextContent(failure.message)
  fireEvent.click(details[0].querySelector('summary')!)
  fireEvent.click(within(details[0]).getByRole('button'))
  await waitFor(() => expect(navigator.clipboard.writeText).toHaveBeenCalledWith(failure.message))
}
describe('PomodoroTimer consumer error contract', () => {
  beforeEach(() => {
    clearToasts()
    Object.values(api).forEach((mock) => {
      mock.mockReset()
    })
    api.load.mockResolvedValue(null)
    api.start.mockResolvedValue(session())
    api.cancel.mockResolvedValue(session('cancelled'))
    api.complete.mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
    })
    const notification = vi.fn()
    Object.assign(notification, { permission: 'granted', requestPermission: vi.fn() })
    vi.stubGlobal('Notification', notification)
    vi.spyOn(console, 'warn').mockImplementation(() => undefined)
  })
  afterEach(() => {
    clearToasts()
    vi.useRealTimers()
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })
  it.each(Object.keys(resources))('shows localized load failure and copied support detail in %s', async (language) => {
    api.load.mockRejectedValue(failure)
    const locale = await mount(language)
    await assertDisplay('focus.pomodoro.loadFailed', locale)
  })
  it.each(Object.keys(resources))('separates a slotted policy message from load detail in %s', async (language) => {
    api.load.mockRejectedValue({ code: 'policy.denied', message: 'timer policy diagnostic' })
    await mount(language)
    await waitFor(() =>
      expect(records()).toEqual([
        expect.objectContaining({
          type: 'error',
          message: policyPrimary[language],
          duration: 5000,
          detail: 'policy.denied: timer policy diagnostic',
        }),
      ]),
    )
    expect(screen.getByRole('alert').querySelector('p')).toHaveTextContent(policyPrimary[language])
    expect(screen.getByRole('alert').querySelector('p')).not.toHaveTextContent('timer policy diagnostic')
    for (const detail of screen.getAllByTestId('support-details')) expect(detail).not.toHaveAttribute('open')
  })
  it('restores the start action after failure and preserves 25/5 arguments', async () => {
    api.start.mockRejectedValueOnce(failure).mockResolvedValueOnce(session())
    const locale = await mount()
    fireEvent.click(screen.getByRole('button'))
    await assertDisplay('focus.pomodoro.startFailed', locale)
    expect(api.start).toHaveBeenCalledWith({ duration_minutes: 25, break_minutes: 5 })
    fireEvent.click(screen.getAllByRole('button')[0])
    await waitFor(() => expect(api.start).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument())
  })
  it('preserves cancel recovery after a failure', async () => {
    api.load.mockResolvedValue(session())
    api.cancel.mockRejectedValueOnce(failure).mockResolvedValueOnce(session('cancelled'))
    const locale = await mount()
    await waitFor(() => expect(document.querySelector('svg text')).toHaveTextContent('25:00'))
    fireEvent.click(screen.getByRole('button'))
    await assertDisplay('focus.pomodoro.cancelFailed', locale)
    fireEvent.click(screen.getAllByRole('button')[0])
    await waitFor(() => expect(api.cancel).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument())
  })
  it('keeps completion failure a 5000ms warning and preserves timer notifications', async () => {
    vi.useFakeTimers()
    api.load.mockResolvedValue(session('running', 1 / 60, 1 / 60))
    api.complete.mockRejectedValue(failure)
    const locale = await mount()
    // Settle the loaded session and its interval before advancing the clock.
    await act(async () => {
      await Promise.resolve()
    })
    expect(document.querySelector('svg text')).toHaveTextContent('00:01')
    expect(api.complete).not.toHaveBeenCalled()
    expect(Notification).not.toHaveBeenCalled()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000)
    })
    expect(document.querySelector('svg text')).toHaveTextContent('00:01')
    expect(api.complete).not.toHaveBeenCalled()
    expect(Notification).toHaveBeenCalledTimes(1)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000)
    })
    expect(api.complete).toHaveBeenCalledTimes(1)
    expect(Notification).toHaveBeenCalledTimes(2)
    expect(Notification.requestPermission).not.toHaveBeenCalled()
    expect(records()).toEqual([
      expect.objectContaining({
        type: 'warning',
        message: locale.t('focus.pomodoro.syncFailed'),
        duration: 5000,
        detail: failure.message,
      }),
    ])
    expect(screen.getByRole('alert').querySelector('p')).toHaveTextContent(locale.t('focus.pomodoro.syncFailed'))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(4999)
    })
    expect(records()).toHaveLength(1)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1)
    })
    expect(records()).toHaveLength(0)
  })
})
