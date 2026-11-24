import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, renderHook, screen, waitFor } from '@testing-library/react'
import { createInstance } from 'i18next'
import { type PropsWithChildren, useEffect } from 'react'
import { I18nextProvider, initReactI18next } from 'react-i18next'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { DurableTaskPanel } from '../../components/DurableTaskPanel'
import { ToastContainer } from '../../components/ui/Toast'
import en from '../../i18n/locales/en.json'
import es from '../../i18n/locales/es.json'
import ja from '../../i18n/locales/ja.json'
import ko from '../../i18n/locales/ko.json'
import zhCN from '../../i18n/locales/zh-CN.json'
import { ContextRecoveryPanel } from '../../tracking-panel/ContextRecoveryPanel'
import { useUpdateGoals } from '../useCoaching'
import { useContextRecovery } from '../useContextRecovery'
import { useDurableTasks } from '../useDurableTasks'
import { useExtensions } from '../useExtensions'
import { useCreateOverride, useDeleteOverride } from '../useRecalibration'
import { clearToasts, useToast } from '../useToast'

// Keep the production API adapters, React Query and display functions. Only the
// native IPC and HTTP transport are replaced, with explicit failure payloads.
const { invoke } = vi.hoisted(() => {
  const invoke = vi.fn()
  Object.defineProperty(window, '__TAURI_INTERNALS__', { value: { invoke }, configurable: true })
  return { invoke }
})

const CATALOGS = { en, ko, ja, 'zh-CN': zhCN, es }
const resources = Object.fromEntries(Object.entries(CATALOGS).map(([key, translation]) => [key, { translation }]))
let language: ReturnType<typeof createInstance>
let queryClient: QueryClient
let fetchMock: ReturnType<typeof vi.fn>

function Providers({ children }: PropsWithChildren) {
  return (
    <I18nextProvider i18n={language}>
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    </I18nextProvider>
  )
}

beforeEach(async () => {
  language = createInstance()
  await language.use(initReactI18next).init({ lng: 'ko', fallbackLng: 'en', resources })
  queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  invoke.mockReset()
  invoke.mockImplementation(async (command: string) => {
    if (command === 'get_web_port') return 10090
    if (command === 'get_local_auth_token') return 'test-local-auth'
    return []
  })
  window.__MAEKON_LOCAL_AUTH__ = 'test-local-auth'
  fetchMock = vi.fn()
  vi.stubGlobal('fetch', fetchMock)
  clearToasts()
})

afterEach(() => {
  act(() => clearToasts())
  queryClient.clear()
  vi.unstubAllGlobals()
})

function useTaskSubject() {
  const state = useDurableTasks()
  return { ...state, settled: !state.loading, retry: state.refresh }
}

function useExtensionSubject() {
  const state = useExtensions()
  return { ...state, settled: !state.loading, retry: state.refresh }
}

function useRecoverySubject() {
  const state = useContextRecovery()
  useEffect(() => {
    void state.generate()
  }, [state.generate])
  return { ...state, settled: state.phase === 'error' || state.phase === 'done', retry: state.generate }
}

const SUBJECTS = [
  {
    name: 'durable tasks',
    useSubject: useTaskSubject,
    command: 'list_task_candidates',
    fallback: 'durableTasks.loadError',
  },
  { name: 'extensions', useSubject: useExtensionSubject, command: 'list_extensions', fallback: 'errors.generic' },
  {
    name: 'context recovery',
    useSubject: useRecoverySubject,
    command: 'request_current_context_suggestions',
    fallback: 'recovery.outcome.errorDetail',
  },
] as const

const FAILURES = [
  {
    name: 'known IPC code',
    cause: { code: 'storage.unavailable', message: 'fixture storage detail' },
    key: 'errors.ipc.storageUnavailable',
    detail: null,
  },
  {
    name: 'unknown code',
    cause: { code: 'future.failed', message: 'fixture diagnostic' },
    detail: 'future.failed: fixture diagnostic',
  },
  { name: 'Error', cause: new Error('fixture diagnostic'), detail: 'fixture diagnostic' },
  { name: 'legacy string', cause: 'fixture diagnostic', detail: 'fixture diagnostic' },
  { name: 'empty string', cause: '', detail: null },
  { name: 'null', cause: null, detail: null },
  { name: 'undefined', cause: undefined, detail: null },
  { name: 'non-string', cause: 42, detail: null },
] as const

describe.each(SUBJECTS)('$name failure state (#13861)', ({ useSubject, command, fallback }) => {
  it.each(FAILURES)('preserves $name as a localized failure, including falsy rejection values', async (failure) => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === command) throw failure.cause
      return []
    })
    const { result } = renderHook(useSubject, { wrapper: Providers })
    await waitFor(() => expect(result.current.settled).toBe(true))
    const key = 'key' in failure ? failure.key : fallback
    expect(result.current.error).toEqual({ text: language.t(key), detail: failure.detail })
    expect(result.current.error?.text).not.toContain('{{')
    expect(result.current.error?.text).not.toContain('fixture diagnostic')
  })

  it('updates all supported languages without another request and recovers on explicit retry', async () => {
    let reject = true
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === command && reject) throw new Error('fixture diagnostic')
      return cmd === 'request_current_context_suggestions' ? { status: 'no_candidate' } : []
    })
    const { result } = renderHook(useSubject, { wrapper: Providers })
    await waitFor(() => expect(result.current.settled).toBe(true))
    const retry = result.current.retry
    const calls = invoke.mock.calls.length
    for (const locale of Object.keys(CATALOGS)) {
      await act(async () => {
        await language.changeLanguage(locale)
      })
      expect(result.current.error?.text).toBe(language.t(fallback))
      expect(result.current.error?.detail).toBe('fixture diagnostic')
      expect(result.current.retry).toBe(retry)
      expect(invoke).toHaveBeenCalledTimes(calls)
    }
    reject = false
    await act(async () => {
      await result.current.retry()
    })
    expect(result.current.error).toBeNull()
    expect(result.current.settled).toBe(true)
    const afterSuccess = invoke.mock.calls.length
    await act(async () => {
      await language.changeLanguage('ko')
    })
    expect(result.current.error).toBeNull()
    expect(invoke).toHaveBeenCalledTimes(afterSuccess)
    reject = true
    await act(async () => {
      await result.current.retry()
    })
    expect(result.current.error?.text).toBe(language.t(fallback))
  })
})

it.each(SUBJECTS.slice(0, 2))('$name ignores an earlier success after a newer load failure', async ({
  useSubject,
  command,
}) => {
  let resolveOld!: (value: unknown[]) => void
  const old = new Promise<unknown[]>((resolve) => {
    resolveOld = resolve
  })
  let count = 0
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd !== command) return []
    if (++count === 1) return old
    throw { code: 'storage.unavailable', message: 'fixture diagnostic' }
  })
  const { result } = renderHook(useSubject, { wrapper: Providers })
  await waitFor(() => expect(count).toBe(1))
  await act(async () => {
    await result.current.retry()
  })
  expect(result.current.error?.text).toBe(ko.errors.ipc.storageUnavailable)
  await act(async () => {
    resolveOld([])
    await old
  })
  expect(result.current.error?.text).toBe(ko.errors.ipc.storageUnavailable)
})

it('does not resurrect a recovery result after reset during an in-flight request', async () => {
  let resolveOld!: (value: { status: string }) => void
  const old = new Promise<{ status: string }>((resolve) => {
    resolveOld = resolve
  })
  invoke.mockReturnValue(old)
  const { result } = renderHook(useContextRecovery, { wrapper: Providers })
  let first!: ReturnType<typeof result.current.generate>
  act(() => {
    first = result.current.generate()
  })
  act(() => {
    result.current.reset()
  })
  await act(async () => {
    resolveOld({ status: 'no_candidate' })
    await first
  })
  expect(result.current.phase).toBe('idle')
  expect(result.current.result).toBeNull()
  await expect(first).resolves.toBeNull()
})

describe('mutation notices through real API adapters', () => {
  it.each(FAILURES)('localizes a goal mutation rejection: $name', async (failure) => {
    invoke.mockRejectedValue(failure.cause)
    const { result } = renderHook(() => ({ mutation: useUpdateGoals(), toast: useToast() }), { wrapper: Providers })
    render(<ToastContainer />, { wrapper: Providers })
    act(() => {
      result.current.mutation.mutate({ deep_work: 180 })
    })
    await waitFor(() => expect(result.current.mutation.isError).toBe(true))
    expect(invoke).toHaveBeenCalledWith('update_regime_goals', { goals: { deep_work: 180 } }, undefined)
    const key = 'key' in failure ? failure.key : 'errors.generic'
    expect(result.current.toast.toasts).toHaveLength(1)
    const message = document.querySelector('[aria-live="polite"] p')
    expect(message).toHaveTextContent(language.t(key))
    expect(message).not.toHaveTextContent('fixture diagnostic')
    expect(result.current.toast.toasts[0].detail ?? null).toBe(failure.detail)
    const toastId = result.current.toast.toasts[0].id
    await act(async () => {
      await language.changeLanguage('en')
    })
    expect(message).toHaveTextContent(language.t(key))
    expect(result.current.toast.toasts.map((toast) => toast.id)).toEqual([toastId])
    expect(invoke).toHaveBeenCalledTimes(1)
  })

  it('translates successful goal saves and preserves both cache invalidations', async () => {
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    const { result } = renderHook(() => ({ mutation: useUpdateGoals(), toast: useToast() }), { wrapper: Providers })
    act(() => {
      result.current.mutation.mutate({ deep_work: 180 })
    })
    await waitFor(() => expect(result.current.mutation.isSuccess).toBe(true))
    expect(result.current.toast.toasts[0].message).toBe(ko.coaching.goalsUpdated)
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['goal-progress'] })
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['settings'] })
  })

  it.each([
    'create',
    'delete',
  ] as const)('preserves the %s override HTTP error code and localizes the toast', async (operation) => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ code: 'storage.unavailable', error: 'fixture storage detail' }), { status: 400 }),
    )
    const { result } = renderHook(
      () => ({ create: useCreateOverride(), remove: useDeleteOverride(), toast: useToast() }),
      { wrapper: Providers },
    )
    render(<ToastContainer />, { wrapper: Providers })
    act(() => {
      if (operation === 'create')
        result.current.create.mutate({ segment_id: 'segment-1', action: { type: 'MARK_AS_NOISE' } })
      else result.current.remove.mutate('override/1')
    })
    await waitFor(() => expect(result.current.toast.toasts).toHaveLength(1))
    const message = document.querySelector('[aria-live="polite"] p')
    expect(message).toHaveTextContent(ko.errors.ipc.storageUnavailable)
    expect(message).not.toHaveTextContent('fixture storage detail')
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe(
      `http://127.0.0.1:10090/api/recalibration/override${operation === 'delete' ? '/override%2F1' : ''}`,
    )
    expect(init.method).toBe(operation === 'create' ? 'POST' : 'DELETE')
    expect(new Headers(init.headers).get('X-Local-Auth')).toBe('test-local-auth')
    await act(async () => {
      await language.changeLanguage('en')
    })
    expect(message).toHaveTextContent(en.errors.ipc.storageUnavailable)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
})

it.each([
  'tasks',
  'recovery',
] as const)('keeps the %s failure diagnostic collapsed in the actual consuming screen', async (surface) => {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === (surface === 'tasks' ? 'list_task_candidates' : 'request_current_context_suggestions')) {
      throw new Error('fixture diagnostic')
    }
    return []
  })
  const { container } = render(
    surface === 'tasks' ? <DurableTaskPanel /> : <ContextRecoveryPanel onBack={() => {}} />,
    { wrapper: Providers },
  )
  await waitFor(() => expect(container.querySelector('p')).not.toBeNull())
  const paragraphs = Array.from(container.querySelectorAll('p'))
    .map((p) => p.textContent)
    .join(' ')
  expect(paragraphs).not.toContain('fixture diagnostic')
  const details = await screen.findByTestId('support-details')
  expect(details).not.toHaveAttribute('open')
  expect(screen.getByTestId('support-details-text')).toHaveTextContent('fixture diagnostic')
})
