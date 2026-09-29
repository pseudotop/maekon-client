// #12559: the Chat page starts on the HTTP surface saved in Settings, not the first in the list.
// #12541 sends the saved key only to that surface, so any other start reads as "not configured".
import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AppSettings, ProviderSurfaceCatalog } from '../../../api/contracts'
import { DEFAULT_PROVIDER_SURFACE_CATALOG } from '../../../api/defaultProviderSurfaceCatalog'
// The app's i18n instance keeps `t` stable; without it the sessions effect re-runs every render.
import '../../../i18n'
import { filterHttpApiSurfaces } from '../utils'
import { useSessionSetup } from './useSessionSetup'

const api = vi.hoisted(() => ({
  fetchProviderSurfaces: vi.fn(),
  fetchSettings: vi.fn(),
}))
vi.mock('../../../api/client', () => api)

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'get_token_usage') {
      return { totalInputTokens: 0, totalOutputTokens: 0, dailyBudget: 0, budgetRemaining: null }
    }
    if (cmd === 'list_ai_sessions') return []
    throw new Error(`unexpected ipc: ${cmd}`)
  }),
}))

const HTTP_SURFACE_IDS = filterHttpApiSurfaces(DEFAULT_PROVIDER_SURFACE_CATALOG).map((surface) => surface.surface_id)
const FIRST = HTTP_SURFACE_IDS[0]
const CONFIGURED = 'provider_surface.openai.direct_api'

function settingsWith(llmApi: { surface_id?: string | null } | null): AppSettings {
  return { ai_provider: { llm_api: llmApi } } as unknown as AppSettings
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((settle) => {
    resolve = settle
  })
  return { promise, resolve }
}

async function startedSurface(settings: Promise<AppSettings>): Promise<string> {
  api.fetchSettings.mockReturnValue(settings)
  const { result } = renderHook(() => useSessionSetup())
  await waitFor(() => expect(result.current.httpSurfaceId).not.toBe(''))
  // Let every pending fetch settle; the first choice must not flip afterwards.
  await act(async () => {})
  return result.current.httpSurfaceId
}

describe('useSessionSetup initial HTTP surface', () => {
  beforeEach(() => {
    api.fetchProviderSurfaces.mockReset().mockResolvedValue(DEFAULT_PROVIDER_SURFACE_CATALOG)
    api.fetchSettings.mockReset()
  })

  it('starts on the surface saved in Settings even when it is not the first', async () => {
    expect(HTTP_SURFACE_IDS).toContain(CONFIGURED)
    expect(CONFIGURED).not.toBe(FIRST)

    expect(await startedSurface(Promise.resolve(settingsWith({ surface_id: CONFIGURED })))).toBe(CONFIGURED)
  })

  it('starts on the first surface when Settings saved none', async () => {
    expect(await startedSurface(Promise.resolve(settingsWith(null)))).toBe(FIRST)
    expect(await startedSurface(Promise.resolve(settingsWith({ surface_id: null })))).toBe(FIRST)
  })

  it('starts on the first surface when the saved surface is not an HTTP surface', async () => {
    const saved = settingsWith({ surface_id: 'provider_surface.openai.managed_oauth' })

    expect(await startedSurface(Promise.resolve(saved))).toBe(FIRST)
  })

  it('starts on the first surface when Settings cannot be read', async () => {
    expect(await startedSurface(Promise.reject(new Error('Settings query failed')))).toBe(FIRST)
  })

  it('waits for the fetched catalog before choosing', async () => {
    // A surface only the fetched catalog has must not lose to the static fallback's first entry.
    const template = DEFAULT_PROVIDER_SURFACE_CATALOG.surfaces.find((surface) => surface.surface_id === CONFIGURED)
    if (!template) throw new Error('fixture surface missing')
    const fetchedOnly = { ...template, surface_id: 'provider_surface.example.direct_api', display_name: 'Example' }
    const catalog = deferred<ProviderSurfaceCatalog>()
    api.fetchProviderSurfaces.mockReturnValue(catalog.promise)
    api.fetchSettings.mockResolvedValue(settingsWith({ surface_id: fetchedOnly.surface_id }))

    const { result } = renderHook(() => useSessionSetup())
    await act(async () => {})
    expect(result.current.httpSurfaceId).toBe('')

    await act(async () => {
      catalog.resolve({
        ...DEFAULT_PROVIDER_SURFACE_CATALOG,
        surfaces: [...DEFAULT_PROVIDER_SURFACE_CATALOG.surfaces, fetchedOnly],
      })
    })
    await waitFor(() => expect(result.current.httpSurfaceId).toBe(fetchedOnly.surface_id))
  })

  it('waits for Settings before choosing', async () => {
    const settings = deferred<AppSettings>()
    api.fetchSettings.mockReturnValue(settings.promise)

    const { result } = renderHook(() => useSessionSetup())
    await act(async () => {})
    expect(result.current.httpSurfaceId).toBe('')

    await act(async () => {
      settings.resolve(settingsWith({ surface_id: CONFIGURED }))
    })
    await waitFor(() => expect(result.current.httpSurfaceId).toBe(CONFIGURED))
  })

  it('keeps a surface the user picked', async () => {
    api.fetchSettings.mockResolvedValue(settingsWith({ surface_id: CONFIGURED }))
    const { result } = renderHook(() => useSessionSetup())
    await waitFor(() => expect(result.current.httpSurfaceId).toBe(CONFIGURED))

    const picked = HTTP_SURFACE_IDS.find((id) => id !== CONFIGURED && id !== FIRST) ?? ''
    act(() => result.current.setHttpSurfaceId(picked))
    await act(async () => {})

    expect(result.current.httpSurfaceId).toBe(picked)
  })
})
