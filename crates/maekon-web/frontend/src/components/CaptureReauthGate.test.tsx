import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiClientError } from '../api/client'
import type { ReauthStatus } from '../api/reauth'
import {
  CaptureReauthControls,
  CaptureReauthDialog,
  CaptureReauthGate,
  useCaptureReauthRecovery,
} from './CaptureReauthGate'

const { authenticateCaptureHistory, getCaptureReauthStatus } = vi.hoisted(() => ({
  authenticateCaptureHistory: vi.fn(),
  getCaptureReauthStatus: vi.fn(),
}))

vi.mock('../api/reauth', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/reauth')>()
  return { ...actual, authenticateCaptureHistory, getCaptureReauthStatus }
})

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, options?: { kind?: string }) => (options?.kind ? `${key}:${options.kind}` : key),
  }),
}))

const pinOnlyStatus: ReauthStatus = {
  enabled: true,
  idle_timeout_secs: 300,
  authenticated: false,
  biometric_available: false,
  biometric_kind: null,
  pin_enrolled: true,
}

const authenticatedStatus: ReauthStatus = {
  ...pinOnlyStatus,
  authenticated: true,
}

function RecoveryProbe({ retry }: { retry: () => Promise<void> }) {
  const { requestCaptureReauth } = useCaptureReauthRecovery()

  return (
    <button
      type="button"
      onClick={() => {
        void requestCaptureReauth(new ApiClientError('auth.reauth_required', 'Re-authentication required', 403), retry)
      }}
    >
      protected action
    </button>
  )
}

describe('CaptureReauthControls', () => {
  beforeEach(() => {
    authenticateCaptureHistory.mockReset()
    getCaptureReauthStatus.mockReset()
  })

  it('authenticates a PIN-only platform and resumes the protected action', async () => {
    const user = userEvent.setup()
    const onAuthenticated = vi.fn()
    authenticateCaptureHistory.mockResolvedValue({ outcome: 'authenticated' })

    render(
      <CaptureReauthControls
        status={pinOnlyStatus}
        onAuthenticated={onAuthenticated}
        onCancel={vi.fn()}
        onGoToSettings={vi.fn()}
      />,
    )

    await user.type(screen.getByLabelText('reauth.pinLabel'), '2468')
    await user.click(screen.getByRole('button', { name: 'reauth.unlock' }))

    await waitFor(() => {
      expect(authenticateCaptureHistory).toHaveBeenCalledWith({ method: 'pin', pin: '2468' })
      expect(onAuthenticated).toHaveBeenCalledOnce()
    })
  })

  it('falls back to the enrolled app PIN when biometrics are unsupported', async () => {
    const user = userEvent.setup()
    authenticateCaptureHistory.mockResolvedValue({ outcome: 'unsupported' })

    render(
      <CaptureReauthControls
        status={{
          ...pinOnlyStatus,
          biometric_available: true,
          biometric_kind: 'Windows Hello',
        }}
        onAuthenticated={vi.fn()}
        onCancel={vi.fn()}
        onGoToSettings={vi.fn()}
      />,
    )

    await user.click(screen.getByRole('button', { name: 'reauth.useBiometric:Windows Hello' }))

    expect(await screen.findByLabelText('reauth.pinLabel')).toBeInTheDocument()
    expect(screen.getByRole('alert')).toHaveTextContent('reauth.errors.biometricUnavailable')
  })

  it('renders action-scoped re-authentication in an accessible dialog', async () => {
    const user = userEvent.setup()
    const onClose = vi.fn()

    render(
      <CaptureReauthDialog
        status={pinOnlyStatus}
        onAuthenticated={vi.fn()}
        onClose={onClose}
        onGoToSettings={vi.fn()}
      />,
    )

    expect(screen.getByRole('dialog', { name: 'reauth.title' })).toBeInTheDocument()
    expect(screen.getByLabelText('reauth.pinLabel')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'reauth.cancel' }))
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('prompts and retries when an already-mounted mutation reports idle re-auth expiry', async () => {
    const user = userEvent.setup()
    const retry = vi.fn(async () => {})
    getCaptureReauthStatus.mockResolvedValueOnce(authenticatedStatus).mockResolvedValueOnce(pinOnlyStatus)
    authenticateCaptureHistory.mockResolvedValue({ outcome: 'authenticated' })

    render(
      <MemoryRouter>
        <CaptureReauthGate>
          <RecoveryProbe retry={retry} />
        </CaptureReauthGate>
      </MemoryRouter>,
    )

    await user.click(await screen.findByRole('button', { name: 'protected action' }))
    expect(await screen.findByRole('dialog', { name: 'reauth.title' })).toBeInTheDocument()

    await user.type(screen.getByLabelText('reauth.pinLabel'), '2468')
    await user.click(screen.getByRole('button', { name: 'reauth.unlock' }))

    await waitFor(() => expect(retry).toHaveBeenCalledOnce())
    expect(screen.queryByRole('dialog', { name: 'reauth.title' })).not.toBeInTheDocument()
  })
})
