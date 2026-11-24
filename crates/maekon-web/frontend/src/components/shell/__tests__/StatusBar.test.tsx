import { screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import i18n from '../../../i18n'
import StatusBar from '../StatusBar'

// Mock useSSE to control connection state
vi.mock('../../../hooks/useSSE', () => ({
  useSSE: vi.fn(() => ({
    status: 'disconnected',
    latestMetrics: null,
    latestFrame: null,
    idleState: null,
    metricsHistory: [],
    connect: vi.fn(),
    disconnect: vi.fn(),
  })),
}))

// Need to import after mock setup to get mock reference
import { useSSE } from '../../../hooks/useSSE'

const mockUseSSE = vi.mocked(useSSE)

describe('StatusBar', () => {
  afterEach(async () => {
    vi.useRealTimers()
    await i18n.changeLanguage('en')
  })

  it('has displayName', () => {
    expect(StatusBar.displayName).toBe('StatusBar')
  })

  it('shows local mode by default when the realtime stream is disconnected', () => {
    renderWithProviders(<StatusBar />)
    expect(screen.getByText(/local mode/i)).toBeInTheDocument()
  })

  it('shows version string', () => {
    renderWithProviders(<StatusBar />)
    expect(screen.getByText('v0.1.0-test')).toBeInTheDocument()
  })

  // #13740: "--" looked broken. Metric cells wait for their first value, and the
  // capture cell says plainly that nothing has been captured yet.
  it('shows no "--" placeholders before the first capture and measurement', () => {
    const { container } = renderWithProviders(<StatusBar />)
    expect(container).not.toHaveTextContent('--')
    expect(screen.getByText('No capture yet')).toBeInTheDocument()
    expect(screen.getByLabelText('Waiting for capture')).toHaveClass('bg-status-connecting')
    expect(container).not.toHaveTextContent('%')
    expect(screen.getByText('v0.1.0-test')).toBeInTheDocument()
  })

  it('labels the capture state in the active language', async () => {
    await i18n.changeLanguage('ko')
    renderWithProviders(<StatusBar />)
    expect(screen.getByText('아직 캡처 없음')).toBeInTheDocument()
    expect(screen.getByLabelText('캡처 대기 중')).toBeInTheDocument()
  })

  it('shows "Connected" when status is connected', () => {
    mockUseSSE.mockReturnValue({
      status: 'connected',
      latestMetrics: null,
      latestFrame: null,
      idleState: null,
      metricsHistory: [],
      connect: vi.fn(),
      disconnect: vi.fn(),
    })
    renderWithProviders(<StatusBar />)
    expect(screen.getByText(/connected/i)).toBeInTheDocument()
  })

  it('shows formatted CPU/RAM when metrics available', () => {
    mockUseSSE.mockReturnValue({
      status: 'connected',
      latestMetrics: {
        timestamp: new Date().toISOString(),
        cpu_usage: 45.2,
        memory_percent: 60,
        memory_used: 8_589_934_592, // 8192MB
        memory_total: 16_000_000_000,
      },
      latestFrame: null,
      idleState: null,
      metricsHistory: [],
      connect: vi.fn(),
      disconnect: vi.fn(),
    })
    renderWithProviders(<StatusBar />)
    expect(screen.getByText('45.2%')).toBeInTheDocument()
    expect(screen.getByText('8.0GB')).toBeInTheDocument()
    expect(screen.queryByText('8192MB')).not.toBeInTheDocument()
  })

  it('shows live capture indicator and latest capture timestamp', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-05-16T02:40:00.000Z'))

    const timestamp = '2026-05-16T02:39:59.500Z'
    mockUseSSE.mockReturnValue({
      status: 'connected',
      latestMetrics: null,
      latestFrame: {
        id: 42,
        timestamp,
        app_name: 'Code',
        window_title: 'main.rs',
        importance: 0.9,
        trigger_type: 'WindowChange',
      },
      idleState: null,
      metricsHistory: [],
      connect: vi.fn(),
      disconnect: vi.fn(),
    })

    renderWithProviders(<StatusBar />)

    expect(screen.getByLabelText('Capture active')).toHaveClass('bg-status-connected')
    const captureTime = screen.getByLabelText('Last capture timestamp')
    expect(captureTime).toHaveAttribute('dateTime', timestamp)
    // A local HH:MM:SS clock time instead of the raw ISO string.
    expect(captureTime.textContent).toMatch(/^\d{2}:\d{2}:\d{2}$/)
    expect(captureTime).not.toHaveTextContent(timestamp)
  })
})
