import { fireEvent, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { TrayDashboard } from './TrayDashboard'

const baseProps = {
  capture: {
    state: 'active' as const,
    capturedCount: 12,
  },
  activeContext: {
    appName: 'Calculator',
    windowTitle: 'Calculator',
    targetLabel: 'display result 46',
  },
  providers: [
    { id: 'server', label: 'Server API', status: 'connected' as const },
    { id: 'llm', label: 'Local LLM', status: 'unavailable' as const },
    { id: 'cli', label: 'CLI bridge', status: 'connected' as const },
  ],
  suggestions: [
    {
      id: 'sug-copy-result',
      title: 'Explain and copy this result',
      source: 'Calculator display',
      confidence: 0.88,
      placement: 'adjacent-popover' as const,
      unread: true,
      requiresConsent: true,
    },
  ],
  privacy: {
    redaction: 'redacted' as const,
    externalEgress: 'blocked' as const,
    auditReady: true,
  },
}

describe('tray dashboard', () => {
  it('renders Maekon tray dashboard context, suggestion queue, provider health, privacy, and audit affordances', () => {
    renderWithProviders(
      <TrayDashboard
        {...baseProps}
        onCaptureNow={vi.fn()}
        onOpenAudit={vi.fn()}
        onOpenSettings={vi.fn()}
        onOpenSuggestion={vi.fn()}
        onOpenTimeline={vi.fn()}
      />,
    )

    expect(screen.getByRole('region', { name: 'Tray dashboard' })).toBeInTheDocument()
    expect(screen.getByText('Active')).toBeInTheDocument()
    expect(screen.getByText('12 captures')).toBeInTheDocument()
    expect(screen.getByText('Calculator')).toBeInTheDocument()
    expect(screen.getByText('display result 46')).toBeInTheDocument()
    expect(screen.getByText('1 unread suggestion')).toBeInTheDocument()
    expect(screen.getByText('Explain and copy this result')).toBeInTheDocument()
    expect(screen.getByText('Consent required')).toBeInTheDocument()
    expect(screen.getByText('External egress blocked')).toBeInTheDocument()
    expect(screen.getByText('Redacted before provider')).toBeInTheDocument()
    expect(screen.getByText('Audit ready')).toBeInTheDocument()
    expect(screen.getByText('Server API')).toBeInTheDocument()
    expect(screen.getByText('Local LLM')).toBeInTheDocument()
    expect(screen.getByText('CLI bridge')).toBeInTheDocument()
  })

  it('opens suggestion review without executing GUI mutation and keeps core quick actions reachable', () => {
    const onCaptureNow = vi.fn()
    const onOpenAudit = vi.fn()
    const onOpenSettings = vi.fn()
    const onOpenSuggestion = vi.fn()
    const onOpenTimeline = vi.fn()

    renderWithProviders(
      <TrayDashboard
        {...baseProps}
        onCaptureNow={onCaptureNow}
        onOpenAudit={onOpenAudit}
        onOpenSettings={onOpenSettings}
        onOpenSuggestion={onOpenSuggestion}
        onOpenTimeline={onOpenTimeline}
      />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'Review suggestion Explain and copy this result' }))
    expect(onOpenSuggestion).toHaveBeenCalledWith('sug-copy-result')

    fireEvent.click(screen.getByRole('button', { name: 'Capture now' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open timeline' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open audit' }))
    fireEvent.click(screen.getByRole('button', { name: 'Open settings' }))

    expect(onCaptureNow).toHaveBeenCalledTimes(1)
    expect(onOpenTimeline).toHaveBeenCalledTimes(1)
    expect(onOpenAudit).toHaveBeenCalledTimes(1)
    expect(onOpenSettings).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole('button', { name: /execute/i })).not.toBeInTheDocument()
  })

  it('keeps the product-first visual order from context to suggestions to privacy instead of becoming a cost tracker', () => {
    renderWithProviders(
      <TrayDashboard
        {...baseProps}
        onCaptureNow={vi.fn()}
        onOpenAudit={vi.fn()}
        onOpenSettings={vi.fn()}
        onOpenSuggestion={vi.fn()}
        onOpenTimeline={vi.fn()}
      />,
    )

    const contextHeading = screen.getByRole('heading', { name: 'Current GUI context' })
    const suggestionsHeading = screen.getByRole('heading', { name: 'Suggestion queue' })
    const privacyHeading = screen.getByRole('heading', { name: 'Privacy and providers' })

    expect(contextHeading.compareDocumentPosition(suggestionsHeading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(suggestionsHeading.compareDocumentPosition(privacyHeading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

    for (const costTrackerCopy of ['Cost', 'tokens', 'Add Account', 'Usage Dashboard', 'Export', 'Open Full Report']) {
      expect(screen.queryByText(costTrackerCopy)).not.toBeInTheDocument()
    }
  })

  it('exposes stable visual oracle region markers that match the tray dashboard manifest contract', () => {
    const { container } = renderWithProviders(
      <TrayDashboard
        {...baseProps}
        onCaptureNow={vi.fn()}
        onOpenAudit={vi.fn()}
        onOpenSettings={vi.fn()}
        onOpenSuggestion={vi.fn()}
        onOpenTimeline={vi.fn()}
      />,
    )

    for (const region of [
      'capture-state',
      'current-gui-context',
      'suggestion-queue',
      'provider-health',
      'privacy-redaction-state',
      'external-egress-state',
      'audit-affordance',
      'quick-actions',
    ]) {
      expect(container.querySelector(`[data-visual-region="${region}"]`)).toBeInTheDocument()
    }
  })
})
