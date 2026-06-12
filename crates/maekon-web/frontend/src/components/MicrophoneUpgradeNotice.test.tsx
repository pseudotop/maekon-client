import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MicrophoneUpgradeNotice } from './MicrophoneUpgradeNotice'

const mockInvoke = vi.fn()
const mockNavigate = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

vi.mock('react-router-dom', () => ({
  useNavigate: () => mockNavigate,
}))

// Return the key verbatim so assertions can match on the i18n key.
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (k: string) => k }),
}))

const BODY_KEY = 'privacy.consent.microphone.upgradeNotice.body'
const CTA_KEY = 'privacy.consent.microphone.upgradeNotice.cta'

describe('MicrophoneUpgradeNotice (#4686)', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockNavigate.mockReset()
  })

  it('shows the one-time banner when the command returns true', async () => {
    mockInvoke.mockResolvedValue(true)
    render(<MicrophoneUpgradeNotice />)
    expect(await screen.findByText(BODY_KEY)).toBeTruthy()
    expect(mockInvoke).toHaveBeenCalledWith('take_microphone_upgrade_notice')
  })

  it('renders nothing when the command returns false', async () => {
    mockInvoke.mockResolvedValue(false)
    const { container } = render(<MicrophoneUpgradeNotice />)
    await waitFor(() => expect(mockInvoke).toHaveBeenCalled())
    expect(container.firstChild).toBeNull()
  })

  it('navigates to /privacy and hides the banner on CTA click', async () => {
    mockInvoke.mockResolvedValue(true)
    render(<MicrophoneUpgradeNotice />)
    fireEvent.click(await screen.findByText(CTA_KEY))
    expect(mockNavigate).toHaveBeenCalledWith('/privacy')
    await waitFor(() => expect(screen.queryByText(BODY_KEY)).toBeNull())
  })

  it('silently renders nothing when the command rejects (standalone/dev)', async () => {
    mockInvoke.mockRejectedValue(new Error('no tauri runtime'))
    const { container } = render(<MicrophoneUpgradeNotice />)
    await waitFor(() => expect(mockInvoke).toHaveBeenCalled())
    expect(container.firstChild).toBeNull()
  })
})
