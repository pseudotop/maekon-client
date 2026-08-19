import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { resetOsPermissionStatusForTest, setOsCapturePermissionBlocked } from '../hooks/useOsPermissionStatus'
import { CapturePermissionNotice } from './CapturePermissionNotice'

const mockFetchStatus = vi.fn()
const mockOpenSettings = vi.fn()

vi.mock('../api/client', () => ({
  fetchDesktopPermissionStatus: (...args: unknown[]) => mockFetchStatus(...args),
  openDesktopPermissionSettings: (...args: unknown[]) => mockOpenSettings(...args),
}))

// Return the key verbatim so assertions can match on the i18n key.
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (k: string) => k }),
}))

const TITLE_KEY = 'capturePermission.bannerTitle'
const OPEN_KEY = 'capturePermission.openSettings'
const RECHECK_KEY = 'capturePermission.recheck'

function grantedSnapshot(state: string) {
  return {
    platform: 'macos',
    accessibility: { state: 'granted', status_reason: null },
    screen_capture: { state, status_reason: null },
    microphone: { state: 'granted', status_reason: null },
    input_monitoring: { state: 'granted', status_reason: null },
    notifications: { state: 'granted', status_reason: null },
  }
}

describe('CapturePermissionNotice (#8686 AC4)', () => {
  beforeEach(() => {
    mockFetchStatus.mockReset()
    mockOpenSettings.mockReset()
    mockOpenSettings.mockResolvedValue(undefined)
  })

  afterEach(() => {
    act(() => resetOsPermissionStatusForTest())
  })

  it('renders nothing while the OS permission is not blocked', () => {
    const { container } = render(<CapturePermissionNotice />)
    expect(container.firstChild).toBeNull()
  })

  it('shows the persistent banner when the permission is revoked', () => {
    render(<CapturePermissionNotice />)
    act(() => setOsCapturePermissionBlocked(true))
    expect(screen.getByText(TITLE_KEY)).toBeTruthy()
    // No dismiss control: the banner stays until the permission returns.
    expect(screen.queryByText(/dismiss/i)).toBeNull()
  })

  it('opens the OS settings deep link for screen capture', () => {
    render(<CapturePermissionNotice />)
    act(() => setOsCapturePermissionBlocked(true))
    fireEvent.click(screen.getByText(OPEN_KEY))
    expect(mockOpenSettings).toHaveBeenCalledWith('screen_capture')
  })

  it('clears the banner when a re-check reports granted', async () => {
    mockFetchStatus.mockResolvedValue(grantedSnapshot('granted'))
    render(<CapturePermissionNotice />)
    act(() => setOsCapturePermissionBlocked(true))
    fireEvent.click(screen.getByText(RECHECK_KEY))
    await waitFor(() => expect(screen.queryByText(TITLE_KEY)).toBeNull())
  })

  it('keeps the banner when a re-check still reports needs_attention', async () => {
    mockFetchStatus.mockResolvedValue(grantedSnapshot('needs_attention'))
    render(<CapturePermissionNotice />)
    act(() => setOsCapturePermissionBlocked(true))
    fireEvent.click(screen.getByText(RECHECK_KEY))
    await waitFor(() => expect(mockFetchStatus).toHaveBeenCalled())
    expect(screen.getByText(TITLE_KEY)).toBeTruthy()
  })

  it('keeps the banner when the re-check probe rejects (standalone/dev)', async () => {
    mockFetchStatus.mockRejectedValue(new Error('no tauri runtime'))
    render(<CapturePermissionNotice />)
    act(() => setOsCapturePermissionBlocked(true))
    fireEvent.click(screen.getByText(RECHECK_KEY))
    await waitFor(() => expect(mockFetchStatus).toHaveBeenCalled())
    expect(screen.getByText(TITLE_KEY)).toBeTruthy()
  })
})
