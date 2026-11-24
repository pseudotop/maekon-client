import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { createInstance } from 'i18next'
import * as React from 'react'
import { I18nextProvider } from 'react-i18next'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
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

import SupportToolsCard from './SupportToolsCard'

const invoke = vi.hoisted(() => vi.fn())
const logCalls = () => invoke.mock.calls.filter(([command]) => command === 'get_runtime_log_snapshot')

vi.mock('../api/client', () => ({
  fetchSupportDiagnostics: vi.fn().mockResolvedValue({
    schema_version: 'support.diagnostics.v1',
    generated_at: '2026-07-12T00:00:00Z',
    health: {
      storage_ok: true,
      frames_dir_configured: true,
      frames_dir_path: 'C:\\Users\\alice\\AppData\\Local\\maekon\\data',
      frames_dir_exists: true,
      config_manager_configured: true,
      automation_controller_configured: true,
      update_control_configured: true,
    },
    provider_cli: [],
    recent_audit_entries: [],
    recent_policy_events: [],
  }),
}))

describe('SupportToolsCard privacy', () => {
  it('opens the canonical Alpha feedback and privacy route', () => {
    const openSpy = vi.spyOn(window, 'open').mockReturnValue({} as Window)
    renderWithProviders(<SupportToolsCard />)

    fireEvent.click(screen.getByRole('button', { name: 'Alpha feedback & privacy' }))

    expect(openSpy).toHaveBeenCalledWith('https://maekon.dev/alpha-feedback', '_blank', 'noopener,noreferrer')
    openSpy.mockRestore()
  })

  it('shows useful frames-directory status without rendering the local path', async () => {
    renderWithProviders(<SupportToolsCard />)

    fireEvent.click(screen.getByRole('button', { name: 'Open Support Details' }))

    await waitFor(() => {
      expect(screen.getByText('support.diagnostics.v1')).toBeInTheDocument()
    })

    expect(screen.getByText('Frames directory')).toBeInTheDocument()
    expect(screen.queryByText(/C:\\Users\\alice/i)).not.toBeInTheDocument()
    expect(screen.getByText('Yes · Yes')).toBeInTheDocument()
  })
})

async function renderNativeSupport(language: string, tauri: boolean, preference: boolean, dev = false) {
  vi.resetModules()
  vi.doUnmock('@tauri-apps/api/core')
  vi.doUnmock('../utils/platform')
  if (tauri) vi.stubGlobal('__TAURI_INTERNALS__', { invoke })
  else expect('__TAURI_INTERNALS__' in window).toBe(false)
  vi.stubEnv('DEV', dev)
  window.localStorage.setItem('maekon-support-developer-details', preference ? '1' : '0')
  // Keep renderer and hook React identities equal across the isolated consumer import.
  vi.doMock('react', () => React)
  const platform = await import('../utils/platform')
  expect(platform.IS_TAURI).toBe(tauri)
  if (tauri) expect('__TAURI_INTERNALS__' in window).toBe(true)
  const { SupportToolsCard: Consumer } = await import('./SupportToolsCard')
  const locale = await testLocale(language)
  render(
    <I18nextProvider i18n={locale}>
      <Consumer />
    </I18nextProvider>,
  )
  // The first card action opens diagnostics; no locale string is used as a selector.
  fireEvent.click(screen.getAllByRole('button')[0])
  await screen.findByText('support.diagnostics.v1')
  return locale
}
describe('SupportToolsCard native log opt-in and error display', () => {
  afterEach(() => {
    vi.unstubAllEnvs()
    vi.unstubAllGlobals()
    vi.doUnmock('react')
    vi.doUnmock('../utils/platform')
    window.localStorage.removeItem('maekon-support-developer-details')
    vi.restoreAllMocks()
  })
  it.each(
    Object.keys(resources).flatMap((language) => ['future.logs', 'policy.denied'].map((code) => ({ language, code }))),
  )('requires opt-in before native logs and preserves partial diagnostics in %s', async ({ language, code }) => {
    invoke
      .mockReset()
      .mockImplementation((command) =>
        command === 'get_runtime_log_snapshot'
          ? Promise.reject({ code, message: 'runtime log diagnostic' })
          : Promise.resolve(null),
      )
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const locale = await renderNativeSupport(language, true, false)
    const toggle = screen.getByRole('switch')
    expect(toggle).toHaveAttribute('aria-checked', 'false')
    expect(logCalls()).toHaveLength(0)
    fireEvent.click(toggle)
    expect(toggle).toHaveAttribute('aria-checked', 'true')
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('get_runtime_log_snapshot', { lineLimit: 200 }, undefined))
    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent(
      code === 'policy.denied' ? policyPrimary[language] : locale.t('settings.supportLogsLoadFailed'),
    )
    const detail = within(alert).getByTestId('support-details')
    expect(detail).not.toHaveAttribute('open')
    expect(within(detail).getByTestId('support-details-text')).toHaveTextContent(`${code}: runtime log diagnostic`)
    expect(alert.querySelector('div.text-content-secondary > p')).not.toHaveTextContent('runtime log diagnostic')
    fireEvent.click(detail.querySelector('summary')!)
    fireEvent.click(within(detail).getByRole('button'))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(`${code}: runtime log diagnostic`))
    expect(screen.getByText('support.diagnostics.v1')).toBeInTheDocument()
    expect(screen.queryByText(/C:\\Users\\alice/i)).not.toBeInTheDocument()
    fireEvent.click(toggle)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })
  it('keeps browser log calls at zero even with opt-in enabled', async () => {
    invoke.mockReset()
    await renderNativeSupport('en', false, true)
    expect(screen.getByRole('switch')).toHaveAttribute('aria-checked', 'true')
    expect(logCalls()).toHaveLength(0)
    expect(screen.getByText('support.diagnostics.v1')).toBeInTheDocument()
  })
  it('preserves the DEV default even when the saved preference is off', async () => {
    invoke.mockReset().mockResolvedValue(null)
    await renderNativeSupport('en', true, false, true)
    expect(screen.getByRole('switch')).toHaveAttribute('aria-checked', 'true')
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('get_runtime_log_snapshot', { lineLimit: 200 }, undefined))
  })
})
