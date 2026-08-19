// #7916 (Epic #7908 T4.2): Tests for GuiPickerPanel — the highlight → confirm →
// ticket → execute GUI HITL picker. Pattern mirrors IntentHintBar.test.tsx:
// renderWithProviders + vi.mock('../../api/client').
import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { confirmGuiSession, createGuiSession, executeGuiSession, highlightGuiSession } from '../../api/client'
import GuiPickerPanel from './GuiPickerPanel'

// Mock the whole client module — only the four GUI-V2 session functions are used.
vi.mock('../../api/client', () => ({
  createGuiSession: vi.fn(),
  highlightGuiSession: vi.fn(),
  confirmGuiSession: vi.fn(),
  executeGuiSession: vi.fn(),
}))

const enabledStatus = { enabled: true } as never
const disabledStatus = { enabled: false } as never

function makeCreateResponse() {
  return {
    schema_version: 'v1',
    capability_token: 'cap-token-abc',
    session: {
      session_id: 'sess-1',
      state: 'Proposed',
      scene: {},
      focus: {},
      candidates: [
        {
          candidate_id: 'cand-1',
          highlighted: false,
          element: { element_id: 'el-1', label: 'Save button', role: 'button' },
        },
        {
          candidate_id: 'cand-2',
          highlighted: false,
          element: { element_id: 'el-2', label: 'Cancel button', role: 'button' },
        },
      ],
      created_at: '2026-07-07T00:00:00Z',
      updated_at: '2026-07-07T00:00:00Z',
      expires_at: '2026-07-07T00:05:00Z',
    },
  } as never
}

function makeTicket() {
  return {
    ticket_id: 'ticket-1',
    session_id: 'sess-1',
    candidate_id: 'cand-1',
    action: { action_type: 'click' as const },
    issued_at: '2026-07-07T00:00:00Z',
    expires_at: '2026-07-07T00:00:30Z',
  }
}

function makeExecuteResponse(succeeded: boolean) {
  return {
    schema_version: 'v1',
    command_id: 'cmd-1',
    ticket: makeTicket(),
    result: { success: succeeded, element: null, verification: null, retry_count: 0, elapsed_ms: 12, error: null },
    outcome: {
      session: makeCreateResponse().session,
      succeeded,
      detail: succeeded ? null : 'sandbox denied',
      steps_completed: succeeded ? 1 : 0,
      total_steps: 1,
    },
  } as never
}

describe('GuiPickerPanel', () => {
  beforeEach(() => {
    vi.mocked(createGuiSession).mockReset()
    vi.mocked(highlightGuiSession).mockReset()
    vi.mocked(confirmGuiSession).mockReset()
    vi.mocked(executeGuiSession).mockReset()
  })

  it('disables the scan button and shows the hint when automation is disabled', async () => {
    renderWithProviders(<GuiPickerPanel status={disabledStatus} />)
    await waitFor(() => {
      expect(screen.getByTestId('gui-picker-scan')).toBeDisabled()
    })
    expect(screen.getByText(/enable automation/i)).toBeInTheDocument()
  })

  it('drives the full highlight → confirm → ticket → execute flow', async () => {
    vi.mocked(createGuiSession).mockResolvedValue(makeCreateResponse())
    vi.mocked(highlightGuiSession).mockResolvedValue({ schema_version: 'v1' } as never)
    vi.mocked(confirmGuiSession).mockResolvedValue({ schema_version: 'v1', ticket: makeTicket() } as never)
    vi.mocked(executeGuiSession).mockResolvedValue(makeExecuteResponse(true))

    renderWithProviders(<GuiPickerPanel status={enabledStatus} />)

    // Step 1: scan → createGuiSession + highlightGuiSession.
    await act(async () => {
      fireEvent.click(screen.getByTestId('gui-picker-scan'))
    })
    await waitFor(() => {
      expect(screen.getByText('Save button')).toBeInTheDocument()
    })
    expect(createGuiSession).toHaveBeenCalledTimes(1)
    // Highlight session is driven with every candidate id (overlay is painted backend-side).
    expect(highlightGuiSession).toHaveBeenCalledWith('sess-1', 'cap-token-abc', {
      candidate_ids: ['cand-1', 'cand-2'],
    })

    // Step 2: confirm the auto-selected first candidate → mint a ticket.
    await act(async () => {
      fireEvent.click(screen.getByTestId('gui-picker-confirm'))
    })
    await waitFor(() => {
      expect(screen.getByTestId('gui-picker-execute')).toBeInTheDocument()
    })
    expect(confirmGuiSession).toHaveBeenCalledWith('sess-1', 'cap-token-abc', {
      candidate_id: 'cand-1',
      action: { action_type: 'click' },
    })

    // Step 3: execute the minted ticket through the existing gates.
    await act(async () => {
      fireEvent.click(screen.getByTestId('gui-picker-execute'))
    })
    await waitFor(() => {
      expect(screen.getByTestId('gui-picker-result')).toBeInTheDocument()
    })
    expect(executeGuiSession).toHaveBeenCalledWith('sess-1', 'cap-token-abc', {
      ticket: expect.objectContaining({ ticket_id: 'ticket-1' }),
    })
    expect(screen.getByText(/action completed/i)).toBeInTheDocument()
  })

  it('surfaces a scan error and does not advance the flow', async () => {
    vi.mocked(createGuiSession).mockRejectedValue(new Error('scene analysis failed'))

    renderWithProviders(<GuiPickerPanel status={enabledStatus} />)
    await act(async () => {
      fireEvent.click(screen.getByTestId('gui-picker-scan'))
    })
    await waitFor(() => {
      expect(screen.getByText('scene analysis failed')).toBeInTheDocument()
    })
    // No candidate list, so confirm never renders.
    expect(screen.queryByTestId('gui-picker-confirm')).not.toBeInTheDocument()
    expect(confirmGuiSession).not.toHaveBeenCalled()
  })

  it('requires text before a type_text action can be confirmed', async () => {
    vi.mocked(createGuiSession).mockResolvedValue(makeCreateResponse())
    vi.mocked(highlightGuiSession).mockResolvedValue({ schema_version: 'v1' } as never)

    renderWithProviders(<GuiPickerPanel status={enabledStatus} />)
    await act(async () => {
      fireEvent.click(screen.getByTestId('gui-picker-scan'))
    })
    await waitFor(() => {
      expect(screen.getByText('Save button')).toBeInTheDocument()
    })

    // Switch to type_text — confirm must be disabled until text is entered.
    const actionSelect = screen.getByLabelText('Action')
    await act(async () => {
      fireEvent.change(actionSelect, { target: { value: 'type_text' } })
    })
    expect(screen.getByTestId('gui-picker-confirm')).toBeDisabled()

    const textInput = screen.getByLabelText('Text to type')
    await act(async () => {
      fireEvent.change(textInput, { target: { value: 'hello' } })
    })
    await waitFor(() => {
      expect(screen.getByTestId('gui-picker-confirm')).not.toBeDisabled()
    })
  })
})
