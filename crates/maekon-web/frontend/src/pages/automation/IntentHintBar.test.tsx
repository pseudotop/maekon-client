// #5705: Tests for IntentHintBar — natural-language automation entry point.
// Pattern: renderWithProviders + vi.mock('../../api/client') (CommandsSection.test.tsx).
import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { executeIntentHint } from '../../api/client'
import IntentHintBar from './IntentHintBar'

// Mock entire api/client module — only executeIntentHint is called by this component.
vi.mock('../../api/client', () => ({
  executeIntentHint: vi.fn(),
}))

// Minimal AutomationStatus shapes used across tests.
const enabledStatus = { enabled: true } as never
const disabledStatus = { enabled: false } as never

function typeHint(text: string) {
  const input = screen.getByRole('textbox')
  fireEvent.change(input, { target: { value: text } })
}

async function clickRun() {
  const btn = screen.getByTestId('intent-hint-run')
  await act(async () => {
    fireEvent.click(btn)
  })
}

// Canonical successful response from standalone.ts:1062-1088
function makeSuccessResponse(hint = 'Click save') {
  return {
    command_id: 'test-cmd-1',
    session_id: 'intent-bar-12345',
    planned_intent: {
      ClickElement: {
        text: hint,
        role: null,
        app_name: null,
        button: 'left',
      },
    },
    result: {
      success: true,
      element: null,
      verification: null,
      retry_count: 0,
      elapsed_ms: 42,
      error: null,
    },
  }
}

describe('IntentHintBar', () => {
  beforeEach(() => {
    vi.mocked(executeIntentHint).mockReset()
  })

  // (1) disabled state
  it('shows disabled hint and disables run button when status.enabled is false', async () => {
    renderWithProviders(<IntentHintBar status={disabledStatus} />)

    await waitFor(() => {
      expect(screen.getByTestId('intent-hint-run')).toBeDisabled()
    })
    // Disabled explanation line (CommandsSection.tsx:333 parity)
    expect(screen.getByText(/enable automation/i)).toBeInTheDocument()
  })

  // (2) empty input does not call API
  it('does not call executeIntentHint when input is empty', async () => {
    renderWithProviders(<IntentHintBar status={enabledStatus} />)
    await clickRun()
    expect(vi.mocked(executeIntentHint)).not.toHaveBeenCalled()
  })

  // (3) submit calls executeIntentHint with the typed hint and a non-empty session_id
  it('calls executeIntentHint with trimmed hint and stable non-empty session_id', async () => {
    vi.mocked(executeIntentHint).mockResolvedValue(makeSuccessResponse() as never)

    renderWithProviders(<IntentHintBar status={enabledStatus} />)

    typeHint('  Click the save button  ')
    await clickRun()

    await waitFor(() => {
      expect(vi.mocked(executeIntentHint)).toHaveBeenCalledOnce()
    })

    const [payload] = vi.mocked(executeIntentHint).mock.calls[0]
    expect(payload.intent_hint).toBe('Click the save button')
    expect(payload.session_id).toBeTruthy()
    expect(payload.session_id.length).toBeGreaterThan(0)
  })

  // (4) success: renders variant + history link
  it('renders planned-intent variant name and history link on success', async () => {
    vi.mocked(executeIntentHint).mockResolvedValue(makeSuccessResponse('open file') as never)

    renderWithProviders(<IntentHintBar status={enabledStatus} />)
    typeHint('open file')
    await clickRun()

    await waitFor(() => {
      // Variant key from the planned_intent object
      expect(screen.getByText('ClickElement', { exact: false })).toBeInTheDocument()
    })
    // History link (Amendment D)
    expect(screen.getByRole('link', { name: /execution history/i })).toBeInTheDocument()
  })

  // (5) result.success=false renders result.error
  it('renders result.error when result.success is false', async () => {
    const failResponse = {
      ...makeSuccessResponse(),
      result: {
        success: false,
        element: null,
        verification: null,
        retry_count: 1,
        elapsed_ms: 100,
        error: 'Element not found on screen',
      },
    }
    vi.mocked(executeIntentHint).mockResolvedValue(failResponse as never)

    renderWithProviders(<IntentHintBar status={enabledStatus} />)
    typeHint('click something')
    await clickRun()

    await waitFor(() => {
      expect(screen.getByText('Element not found on screen')).toBeInTheDocument()
    })
  })

  // (6) rejected with 'External LLM blocked' renders consent guidance with settings link
  it('renders consent guidance when error contains "External LLM blocked"', async () => {
    vi.mocked(executeIntentHint).mockRejectedValue(
      new Error('External LLM blocked: consent not granted for cli-subscription provider'),
    )

    renderWithProviders(<IntentHintBar status={enabledStatus} />)
    typeHint('do something')
    await clickRun()

    await waitFor(() => {
      // Amendment B: blocked-consent guidance text
      expect(screen.getByText(/blocked by privacy settings/i)).toBeInTheDocument()
    })
    // Link to /settings (privacy/consent section)
    const link = screen.getByRole('link', { name: /settings/i })
    expect(link).toHaveAttribute('href', '/settings')
  })

  // (7) rejected with 'IntentPlanner is not configured' renders setup guidance
  it('renders setup guidance when error contains "IntentPlanner"', async () => {
    vi.mocked(executeIntentHint).mockRejectedValue(
      new Error('IntentPlanner is not configured: no LLM provider available'),
    )

    renderWithProviders(<IntentHintBar status={enabledStatus} />)
    typeHint('do something')
    await clickRun()

    await waitFor(() => {
      // Amendment B: setup guidance for config-missing planner error
      expect(screen.getByText(/LLM provider not configured/i)).toBeInTheDocument()
    })
    // Link to /settings/ai-automation (CommandsSection.tsx:314 precedent)
    const link = screen.getByRole('link', { name: /settings/i })
    expect(link).toHaveAttribute('href', '/settings/ai-automation')
  })

  // (8) generic rejection renders check-history copy
  it('renders check-history copy for generic errors', async () => {
    vi.mocked(executeIntentHint).mockRejectedValue(new Error('Network error: connection refused'))

    renderWithProviders(<IntentHintBar status={enabledStatus} />)
    typeHint('do something')
    await clickRun()

    await waitFor(() => {
      // Amendment B: generic error includes check-history reminder
      expect(screen.getByText(/may still have executed/i)).toBeInTheDocument()
    })
  })

  // Amendment C: honest no-confirm caption always visible for AUTO policy
  it('renders the runs-immediately caption when confirmation_policy is AUTO', () => {
    renderWithProviders(<IntentHintBar status={enabledStatus} />)
    expect(screen.getByText(/runs immediately under strict sandbox/i)).toBeInTheDocument()
  })

  // #5775 FLAG: caption branches on confirmation_policy
  it('renders requires-confirmation caption when confirmation_policy is CONFIRM', () => {
    const confirmStatus = { ...enabledStatus, confirmation_policy: 'CONFIRM' } as never
    renderWithProviders(<IntentHintBar status={confirmStatus} />)
    expect(screen.getByText(/requires your confirmation before running/i)).toBeInTheDocument()
    expect(screen.queryByText(/runs immediately/i)).not.toBeInTheDocument()
  })

  it('renders blocked-by-policy caption when confirmation_policy is BLOCK', () => {
    const blockStatus = { ...enabledStatus, confirmation_policy: 'BLOCK' } as never
    renderWithProviders(<IntentHintBar status={blockStatus} />)
    expect(screen.getByText(/automation execution is disabled by policy/i)).toBeInTheDocument()
    expect(screen.queryByText(/runs immediately/i)).not.toBeInTheDocument()
  })

  it('renders the runs-immediately caption when confirmation_policy is absent (undefined)', () => {
    // Older server payloads without confirmation_policy — should fall back to AUTO caption.
    const noPolicy = { ...enabledStatus } as never
    renderWithProviders(<IntentHintBar status={noPolicy} />)
    expect(screen.getByText(/runs immediately under strict sandbox/i)).toBeInTheDocument()
  })
})
