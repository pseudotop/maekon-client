import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import i18n from '../../i18n'
import AudioTab from './AudioTab'
import { makeDefaultFormData } from './stories-utils'

const controls = vi.hoisted(() => ({ compiled: true, invoke: vi.fn(), listen: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => controls.invoke(...args) }))
vi.mock('@tauri-apps/api/event', () => ({ listen: (...args: unknown[]) => controls.listen(...args) }))
vi.mock('../settings/SettingsFormContext', () => ({
  useSettingsFormContext: () => ({
    form: {
      formData: { ...makeDefaultFormData(), audio: { enabled: true, model_size: 'base' } },
      setFormData: vi.fn(),
    },
    data: { featureCapabilities: { audio_compiled: controls.compiled } },
  }),
}))

const messages = [
  ['en', 'Something went wrong. Try again.'],
  ['ko', '문제가 발생했습니다. 다시 시도하세요.'],
  ['ja', '問題が発生しました。もう一度お試しください。'],
  ['zh-CN', '出现了问题。请重试。'],
  ['es', 'Algo salió mal. Inténtelo de nuevo.'],
] as const
const ready = {
  enabled: true,
  selected_model: 'base',
  model_status: { state: 'ready', path: '/model', size_bytes: 12 },
  stt_provider_loaded: true,
}
const events = new Map<string, (event: { payload: unknown }) => void>()
const unlisten = vi.fn()

beforeEach(() => {
  controls.compiled = true
  controls.invoke.mockReset()
  controls.listen.mockReset().mockImplementation(async (name, callback) => {
    events.set(name, callback)
    return unlisten
  })
  events.clear()
  unlisten.mockClear()
  vi.spyOn(window, 'confirm').mockReturnValue(true)
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: (...args: unknown[]) => controls.invoke(...args),
  }
})
afterEach(async () => {
  vi.restoreAllMocks()
  await i18n.changeLanguage('en')
})

describe('Audio error displays and capability boundary (#13756)', () => {
  it.each(
    messages,
  )('localizes download failures and keeps typed model diagnostics collapsed in %s', async (language, generic) => {
    await i18n.changeLanguage(language)
    let downloads = 0
    controls.invoke.mockImplementation(async (command) => {
      if (command === 'get_audio_status')
        return { ...ready, model_status: { state: 'error', message: 'model integrity diagnostic' } }
      if (command === 'download_whisper_model' && ++downloads === 1)
        throw { code: 'future.audio', message: 'download diagnostic' }
    })
    renderWithProviders(<AudioTab />)
    const modelDetail = await screen.findByTestId('support-details')
    expect(modelDetail.previousElementSibling).toHaveTextContent(generic)
    expect(modelDetail).not.toHaveAttribute('open')
    expect(within(modelDetail).getByTestId('support-details-text')).toHaveTextContent('model integrity diagnostic')
    const buttons = screen.getAllByRole('button')
    const download = buttons.find((button) => button.className.includes('bg-brand'))!
    expect(download).toBeEnabled()
    fireEvent.click(download)
    await waitFor(() => expect(controls.invoke).toHaveBeenCalledWith('download_whisper_model', { modelSize: 'base' }))
    await waitFor(() => expect(screen.getAllByTestId('support-details')).toHaveLength(2))
    const actionDetail = screen.getAllByTestId('support-details')[1]
    expect(actionDetail.previousElementSibling).toHaveTextContent(generic)
    expect(actionDetail.previousElementSibling).not.toHaveTextContent('download diagnostic')
    expect(within(actionDetail).getByTestId('support-details-text')).toHaveTextContent(
      'future.audio: download diagnostic',
    )
    fireEvent.click(download)
    await waitFor(() => expect(downloads).toBe(2))
    expect(screen.getAllByTestId('support-details')).toHaveLength(1)
  })

  it.each([
    'reload_stt_engine',
    'delete_whisper_model',
    'cancel_model_download',
  ])('reports %s failure and retries the same action', async (command) => {
    let attempts = 0
    const status =
      command === 'cancel_model_download'
        ? { ...ready, model_status: { state: 'downloading', progress_pct: 1, bytes_downloaded: 1, total_bytes: 100 } }
        : ready
    controls.invoke.mockImplementation(async (name) => {
      if (name === 'get_audio_status') return status
      if (name === command && ++attempts === 1) throw new Error('action diagnostic')
    })
    renderWithProviders(<AudioTab />)
    await waitFor(() => expect(controls.invoke).toHaveBeenCalledWith('get_audio_status', undefined))
    const action =
      command === 'cancel_model_download'
        ? 'settings.audio.cancel'
        : command === 'delete_whisper_model'
          ? 'settings.audio.delete'
          : 'settings.audio.reload'
    const button = await screen.findByRole('button', { name: i18n.t(action) })
    fireEvent.click(button)
    const detail = await screen.findByTestId('support-details')
    expect(detail.previousElementSibling).toHaveTextContent(messages[0][1])
    expect(detail).not.toHaveAttribute('open')
    expect(attempts).toBe(1)
    fireEvent.click(button)
    await waitFor(() => expect(attempts).toBe(2))
    expect(screen.queryByTestId('support-details')).toBeNull()
  })

  it('blocks model mutations when audio is not compiled, then proves a compiled build invokes them', async () => {
    controls.compiled = false
    controls.invoke.mockResolvedValue(ready)
    const first = renderWithProviders(<AudioTab />)
    await screen.findByRole('button', { name: i18n.t('settings.audio.reload') })
    for (const button of screen.getAllByRole('button')) {
      expect(button).toBeDisabled()
      fireEvent.click(button)
    }
    await vi.dynamicImportSettled()
    expect(controls.invoke.mock.calls.filter(([name]) => name !== 'get_audio_status')).toEqual([])
    first.unmount()
    controls.compiled = true
    renderWithProviders(<AudioTab />)
    fireEvent.click(await screen.findByRole('button', { name: i18n.t('settings.audio.reload') }))
    await waitFor(() => expect(controls.invoke).toHaveBeenCalledWith('reload_stt_engine', undefined))
  })

  it('re-reads completion/error events and releases all listeners on unmount', async () => {
    controls.invoke.mockResolvedValue(ready)
    const view = renderWithProviders(<AudioTab />)
    await waitFor(() => expect(events.size).toBe(3))
    const before = controls.invoke.mock.calls.length
    events.get('audio-model-error')?.({ payload: {} })
    events.get('audio-model-complete')?.({ payload: {} })
    await waitFor(() => expect(controls.invoke.mock.calls.length).toBe(before + 2))
    view.unmount()
    expect(unlisten).toHaveBeenCalledTimes(3)
  })
  it.each([
    ['ja', 'ポリシーにより拒否されました'],
    ['zh-CN', '被策略拒绝'],
    ['es', 'Denegado por la política'],
  ])('uses the actual policy.denied catalog sentence for an Audio action in %s', async (language, primary) => {
    await i18n.changeLanguage(language)
    let attempts = 0
    controls.invoke.mockImplementation(async (command) => {
      if (command === 'get_audio_status') return ready
      if (command === 'download_whisper_model' && ++attempts === 1)
        throw { code: 'policy.denied', message: 'known audio diagnostic' }
    })
    renderWithProviders(<AudioTab />)
    const download = await screen.findByRole('button', { name: i18n.t('settings.audio.redownload') })
    fireEvent.click(download)
    const details = await screen.findByTestId('support-details')
    expect(attempts).toBe(1)
    expect(details.previousElementSibling).toHaveTextContent(primary)
    expect(details.previousElementSibling).not.toHaveTextContent('known audio diagnostic')
    expect(details).not.toHaveAttribute('open')
    expect(within(details).getByTestId('support-details-text')).toHaveTextContent(
      'policy.denied: known audio diagnostic',
    )
    await waitFor(() => expect(download).toBeEnabled())
    fireEvent.click(download)
    await waitFor(() => expect(attempts).toBe(2))
    expect(screen.queryByTestId('support-details')).toBeNull()
  })
})
