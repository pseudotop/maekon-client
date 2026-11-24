import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { clearToasts } from '../../hooks/useToast'
import i18n from '../../i18n'
import { ToastContainer } from '../ui/Toast'
import { LoginForm } from './LoginForm'

const invoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }))
const signedIn = { server_feature: true, authenticated: true, identifier: 'person', organization_id: 'org' }
beforeEach(() => {
  invoke.mockReset()
  ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: (...args: unknown[]) => invoke(...args),
  }
})
afterEach(async () => {
  clearToasts()
  vi.restoreAllMocks()
  await i18n.changeLanguage('en')
})

function mount(onAuthenticated = vi.fn()) {
  const view = renderWithProviders(
    <>
      <LoginForm onAuthenticated={onAuthenticated} />
      <ToastContainer />
    </>,
  )
  const form = view.container.querySelector('form')!
  const password = form.querySelector('input[type="password"]')!
  fireEvent.change(form.querySelector('input[autocomplete="username"]')!, { target: { value: 'person' } })
  fireEvent.change(password, { target: { value: 'typed-password' } })
  fireEvent.change(form.querySelector('input[autocomplete="organization"]')!, { target: { value: 'org' } })
  return { form, password, onAuthenticated }
}

describe('Login failure display with the existing auth contract (#13756)', () => {
  it.each([
    ['en', 'Something went wrong. Try again.'],
    ['ko', '문제가 발생했습니다. 다시 시도하세요.'],
    ['ja', '問題が発生しました。もう一度お試しください。'],
    ['zh-CN', '出现了问题。请重试。'],
    ['es', 'Algo salió mal. Inténtelo de nuevo.'],
  ])('keeps unknown diagnostics collapsed and retries without changing credential keys in %s', async (language, generic) => {
    await i18n.changeLanguage(language)
    invoke
      .mockRejectedValueOnce({ code: 'future.auth', message: 'authentication transport diagnostic' })
      .mockResolvedValueOnce(signedIn)
    const { form, password, onAuthenticated } = mount()
    fireEvent.submit(form)
    const detail = await screen.findByTestId('support-details')
    expect(invoke).toHaveBeenNthCalledWith(1, 'login', {
      identifier: 'person',
      password: 'typed-password',
      organizationId: 'org',
    })
    expect(detail.previousElementSibling).toHaveTextContent(generic)
    expect(detail.previousElementSibling).not.toHaveTextContent('authentication transport diagnostic')
    expect(detail).not.toHaveAttribute('open')
    expect(within(detail).getByTestId('support-details-text')).toHaveTextContent(
      'future.auth: authentication transport diagnostic',
    )
    expect(password).toHaveValue('typed-password')
    expect(onAuthenticated).not.toHaveBeenCalled()
    fireEvent.submit(form)
    await waitFor(() => expect(onAuthenticated).toHaveBeenCalledWith(signedIn))
    expect(invoke).toHaveBeenCalledTimes(2)
    expect(password).toHaveValue('')
  })

  it('retains the form-specific rejected-credential lookup instead of the generic auth mapping', async () => {
    invoke.mockRejectedValue({
      code: 'auth.failed',
      message: 'Authentication error [auth.failed]: login failure (401 Unauthorized): invalid',
    })
    const { form } = mount()
    fireEvent.submit(form)
    await waitFor(() => expect(screen.getByText(/Check your identifier/)).toBeInTheDocument())
    expect(screen.queryByTestId('support-details')).toBeNull()
    expect(invoke).toHaveBeenCalledTimes(1)
  })

  it('clears the password before an onAuthenticated failure and releases the submit control', async () => {
    invoke.mockResolvedValue(signedIn)
    const callback = vi.fn().mockRejectedValue(new Error('status refresh diagnostic'))
    const { form, password } = mount(callback)
    fireEvent.submit(form)
    const detail = await screen.findByTestId('support-details')
    expect(callback).toHaveBeenCalledWith(signedIn)
    expect(password).toHaveValue('')
    expect(detail.previousElementSibling).toHaveTextContent('Something went wrong. Try again.')
    expect(within(detail).getByTestId('support-details-text')).toHaveTextContent('status refresh diagnostic')
    expect(within(form).getByRole('button')).toBeEnabled()
  })
})
