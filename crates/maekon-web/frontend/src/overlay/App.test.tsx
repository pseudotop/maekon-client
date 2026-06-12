import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { I18nextProvider } from 'react-i18next'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import i18n from '../i18n'
import OverlayApp from './App'
import type { SuggestionGuiAnchorPayload, SuggestionSurfacePlacement, SuggestionViewDto } from './types'

type ListenerCallback = (event: { payload: unknown }) => void

const mockInvoke = vi.fn()
let mockWindowLabel = 'magic-overlay'
const listeners = new Map<string, ListenerCallback>()

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    label: mockWindowLabel,
  }),
}))

interface SuggestionReplayFixture {
  tc_id: string
  suggestions: SuggestionViewDto[]
  expected: {
    visible_proposal_titles: string[]
    policy_gate_action_labels: string[]
    audit_safe_feedback_fields: string[]
  }
  privacy: {
    action_not_auto_executed: boolean
    disallowed_feedback_fields: string[]
  }
}

interface CalculatorSuggestionReplayFixture {
  tc_ids: string[]
  native_context: {
    active_app: string
    bundle_id: string
    active_process: {
      process_name: string
      bundle_id: string
      pid: number | null
      screen_id: string
      window_title: string
      window_bounds: { x: number; y: number; width: number; height: number }
    }
    display_value: string
    expression: string
    target_entity: {
      entity_id: string
      kind: string
      label: string
      value: string
      bounds: { x: number; y: number; width: number; height: number }
      center: { x: number; y: number }
      confidence: number
      sources: string[]
    }
    highlight: {
      target_entity_id: string
      bounds: { x: number; y: number; width: number; height: number }
    }
  }
  suggestions: SuggestionViewDto[]
  expected: {
    visible_proposal_titles: string[]
    target_app: string
    target_entity_label: string
    target_display_value: string
    policy_gate_action_labels: string[]
    forbidden_preconsent_commands: string[]
    feedback_payload_fields: string[]
  }
  privacy: {
    action_not_auto_executed: boolean
    host_input_used: boolean
    raw_gui_context_not_in_feedback: string[]
  }
}

function containsRect(
  outer: { x: number; y: number; width: number; height: number },
  inner: { x: number; y: number; width: number; height: number },
) {
  return (
    inner.x >= outer.x &&
    inner.y >= outer.y &&
    inner.x + inner.width <= outer.x + outer.width &&
    inner.y + inner.height <= outer.y + outer.height
  )
}

interface CalculatorClarificationFixture {
  tc_ids: string[]
  native_context: CalculatorSuggestionReplayFixture['native_context']
  instruction: string
  suggestions: SuggestionViewDto[]
  expected: {
    clarification_title: string
    visible_target_app: string
    forbidden_action_labels: string[]
    allowed_labels: string[]
    forbidden_preconsent_commands: string[]
  }
}

interface ProviderUnavailableFallbackFixture {
  tc_ids: string[]
  provider_failure: {
    code: string
    message: string
    raw_screen_context_fragments: string[]
  }
  local_fallback_suggestions: SuggestionViewDto[]
  expected: {
    generic_error_message: string
    retry_label: string
    retry_pending_label: string
    fallback_title: string
    forbidden_provider_commands: string[]
  }
}

interface SuggestionPanelStateFixture {
  tc_ids: string[]
  initial_suggestions: SuggestionViewDto[]
  recovered_suggestions: SuggestionViewDto[]
  provider_failure: {
    message: string
  }
  expected: {
    loading_message: string
    empty_message: string
    generic_error_message: string
    retry_label: string
    recovered_title: string
    closed_panel_class: string
    open_panel_class: string
    raw_context_fragment: string
  }
}

interface SensitiveGuiRedactionFixture {
  tc_ids: string[]
  gui_context: {
    active_app: string
    bundle_id: string
    raw_sensitive_fragments: string[]
    redaction_classes: string[]
  }
  suggestions: SuggestionViewDto[]
  expected: {
    redacted_title: string
    visible_redacted_fragments: string[]
    feedback_payload_fields: string[]
    forbidden_feedback_fields: string[]
  }
  privacy: {
    prompt_redacted_before_display: boolean
    audit_metadata_only: {
      redaction_classes: string[]
      raw_value_persisted: boolean
    }
  }
}

interface KeyboardA11yFlowFixture {
  tc_ids: string[]
  suggestions: SuggestionViewDto[]
  expected: {
    panel_label: string
    closed_panel_class: string
    actions: Array<{
      title: string
      button: string
      command: string
      suggestion_id: string
      action?: 'accept' | 'reject' | 'defer'
      snooze_minutes?: number
    }>
  }
}

interface MultiAppGuiSuggestionFixture {
  tc_ids: string[]
  scenarios: Array<{
    key: string
    placement: SuggestionSurfacePlacement
    native_context: SuggestionGuiAnchorPayload
    expected: {
      breadcrumb: string
      visible_title: string
      suggestion_id: string
      hidden_titles: string[]
      forbidden_preconsent_commands: string[]
    }
  }>
  suggestions: SuggestionViewDto[]
}

interface EndToEndSuggestionWorkflowFixture {
  tc_ids: string[]
  workflow: {
    steps: string[]
    capture_inputs: string[]
    privacy_gates: {
      live_provider_called: boolean
      action_not_auto_executed: boolean
      confirmation_required: boolean
    }
  }
  native_context: SuggestionGuiAnchorPayload
  suggestions: SuggestionViewDto[]
  expected: {
    contour_marker_label: string
    side_panel_placement: SuggestionSurfacePlacement
    breadcrumb: string
    proposal_title: string
    policy_gate_label: string
    forbidden_preconsent_commands: string[]
    feedback_payload_fields: string[]
    success_toast: string
  }
  privacy: {
    host_input_used: boolean
    host_mutation: boolean
    guest_mutation: boolean
    raw_context_not_in_feedback: string[]
  }
}

interface SuggestionRumReplayTrailFixture {
  tc_ids: string[]
  workflow: {
    steps: string[]
    rum_phases: string[]
    privacy_gates: {
      metadata_only: boolean
      raw_context_included: boolean
      action_not_auto_executed: boolean
    }
  }
  native_context: SuggestionGuiAnchorPayload
  suggestions: SuggestionViewDto[]
  expected: {
    marker_label: string
    breadcrumb: string
    proposal_title: string
    replay_trail_label: string
    replay_step_labels: string[]
    replay_payload_fields: string[]
    forbidden_preconsent_commands: string[]
    forbidden_replay_fragments: string[]
  }
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((event: string, callback: ListenerCallback) => {
    listeners.set(event, callback)
    return Promise.resolve(vi.fn())
  }),
}))

function suggestionFixture(id: string, title: string): SuggestionViewDto {
  return {
    id,
    title,
    body: `Confirm ${title} before closing the TC.`,
    priority: 'medium',
    category: null,
    source: 'local',
    confidence_score: 0.91,
    created_at: '2026-05-16T00:00:00Z',
    is_read: false,
    reasoning: 'A GUI click should reach native suggestion IPC.',
  }
}

function loadOverlayFixture<TFixture>(fileName: string): TFixture {
  const testDir = dirname(fileURLToPath(import.meta.url))
  const fixturePath = resolve(testDir, './__fixtures__/suggestions', fileName)
  return JSON.parse(readFileSync(fixturePath, 'utf8')) as TFixture
}

function loadSuggestionReplayFixture(): SuggestionReplayFixture {
  return loadOverlayFixture<SuggestionReplayFixture>('gui-suggestion-record-replay.json')
}

function loadCalculatorSuggestionReplayFixture(): CalculatorSuggestionReplayFixture {
  return loadOverlayFixture<CalculatorSuggestionReplayFixture>('calculator-gui-suggestion-record-replay.json')
}

function loadCalculatorClarificationFixture(): CalculatorClarificationFixture {
  return loadOverlayFixture<CalculatorClarificationFixture>('calculator-ambiguous-target-clarification.json')
}

function loadProviderUnavailableFallbackFixture(): ProviderUnavailableFallbackFixture {
  return loadOverlayFixture<ProviderUnavailableFallbackFixture>('provider-unavailable-fallback.json')
}

function loadSuggestionPanelStateFixture(): SuggestionPanelStateFixture {
  return loadOverlayFixture<SuggestionPanelStateFixture>('panel-state-stability.json')
}

function loadSensitiveGuiRedactionFixture(): SensitiveGuiRedactionFixture {
  return loadOverlayFixture<SensitiveGuiRedactionFixture>('sensitive-gui-redaction.json')
}

function loadKeyboardA11yFlowFixture(): KeyboardA11yFlowFixture {
  return loadOverlayFixture<KeyboardA11yFlowFixture>('keyboard-a11y-flow.json')
}

function loadMultiAppGuiSuggestionFixture(): MultiAppGuiSuggestionFixture {
  return loadOverlayFixture<MultiAppGuiSuggestionFixture>('multi-app-gui-suggestion-record-replay.json')
}

function loadTargetScopedGuiSuggestionPriorityFixture(): MultiAppGuiSuggestionFixture {
  return loadOverlayFixture<MultiAppGuiSuggestionFixture>('target-scoped-gui-suggestion-priority.json')
}

function loadEndToEndSuggestionWorkflowFixture(): EndToEndSuggestionWorkflowFixture {
  return loadOverlayFixture<EndToEndSuggestionWorkflowFixture>('calculator-e2e-workflow-record-replay.json')
}

function loadSuggestionRumReplayTrailFixture(): SuggestionRumReplayTrailFixture {
  return loadOverlayFixture<SuggestionRumReplayTrailFixture>('calculator-rum-replay-trail.json')
}

async function focusByTab(user: ReturnType<typeof userEvent.setup>, target: HTMLElement, maxTabs = 80) {
  for (let index = 0; index < maxTabs; index += 1) {
    if (document.activeElement === target) return
    await user.tab()
  }
  expect(document.activeElement).toBe(target)
}

async function flushAsyncEffects(turns = 8) {
  for (let index = 0; index < turns; index += 1) {
    await act(async () => {
      await Promise.resolve()
    })
  }
}

describe('overlay app', () => {
  beforeEach(() => {
    listeners.clear()
    mockWindowLabel = 'magic-overlay'
    window.localStorage.clear()
    mockInvoke.mockReset()
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve([])
      return Promise.resolve(undefined)
    })
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      value: { invoke: mockInvoke },
    })
  })

  it('renders a persistent tracking border while capture is active and the indicator is visible', async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      const border = screen.getByTestId('tracking-border')
      expect(border).toBeInTheDocument()
      expect(border).toHaveClass('border-[3px]')
      expect(border).not.toHaveClass('bg-brand-signal')
    })
  })

  it.each([
    ['tracking-border-top', 'top-0', 'h-[3px]'],
    ['tracking-border-bottom', 'bottom-0', 'h-[3px]'],
    ['tracking-border-left', 'left-0', 'w-[3px]'],
    ['tracking-border-right', 'right-0', 'w-[3px]'],
  ])('renders only the matching edge inside %s windows', async (label, edgeClass, sizeClass) => {
    mockWindowLabel = label

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    const border = await screen.findByTestId('tracking-border')
    expect(border).toHaveClass(edgeClass)
    expect(border).toHaveClass(sizeClass)
    expect(border).toHaveClass('bg-brand-signal')
    expect(border).not.toHaveClass('border-[3px]')
  })

  it.each([
    'tracking-border-top',
    'tracking-border-bottom',
    'tracking-border-left',
    'tracking-border-right',
  ])('keeps %s windows off the overlay event bridge', async (label) => {
    mockWindowLabel = label

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    expect(await screen.findByTestId('tracking-border')).toBeInTheDocument()

    await flushAsyncEffects()

    expect(listeners.size).toBe(0)
    expect(mockInvoke).not.toHaveBeenCalled()
    expect(screen.queryByTestId('pointer-context-highlight')).not.toBeInTheDocument()
  })

  it('renders pointer-context highlight updates while capture evidence is active', async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:pointer-context-update')).toBe(true)
    })

    act(() => {
      listeners.get('overlay:pointer-context-update')?.({
        payload: {
          enabled: true,
          x: 42,
          y: 64,
          click_count: 3,
          click_pulse: true,
          reduced_motion: false,
          ttl_ms: 1200,
          sample_rate_hz: 30,
        },
      })
    })

    expect(screen.getByTestId('pointer-context-highlight')).toBeInTheDocument()
    expect(screen.getByTestId('pointer-context-halo')).toHaveStyle({ left: '42px', top: '64px' })
    expect(screen.getByTestId('pointer-context-click-ripple')).toBeInTheDocument()
  })

  it('clears pointer-context highlight when the backend disables the transient layer', async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:pointer-context-update')).toBe(true)
    })

    act(() => {
      listeners.get('overlay:pointer-context-update')?.({
        payload: {
          enabled: true,
          x: 42,
          y: 64,
          click_count: 3,
          click_pulse: false,
          reduced_motion: false,
          ttl_ms: 900,
          sample_rate_hz: 30,
        },
      })
    })
    expect(screen.getByTestId('pointer-context-highlight')).toBeInTheDocument()

    act(() => {
      listeners.get('overlay:pointer-context-update')?.({
        payload: {
          enabled: false,
          x: null,
          y: null,
          click_count: 3,
          click_pulse: false,
          reduced_motion: false,
          ttl_ms: 900,
          sample_rate_hz: 30,
        },
      })
    })

    expect(screen.queryByTestId('pointer-context-highlight')).not.toBeInTheDocument()
  })

  it('uses an opaque token-backed surface for the suggestions panel', async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    const panel = await screen.findByLabelText('Suggestions panel')
    expect(panel).toHaveClass('bg-surface-overlay')
    expect(panel).toHaveClass('text-content')
    expect(panel.className).not.toContain('/90')
  })

  it('defines the design tokens consumed by overlay-only Tailwind classes', () => {
    const css = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), './index.css'), 'utf8')

    for (const token of [
      '--surface-base:',
      '--surface-overlay:',
      '--content:',
      '--content-secondary:',
      '--border-muted:',
      '--hover:',
    ]) {
      expect(css).toContain(token)
    }
  })

  it('opens the suggestions panel from an explicit GUI request event', async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    await waitFor(() => {
      expect(screen.getByLabelText('Suggestions panel')).toHaveClass('translate-x-0')
    })
  })

  it('submits accept, reject, and defer feedback from GUI action buttons and refreshes the list', async () => {
    let pendingSuggestions = [
      suggestionFixture('gui-suggestion-accept', 'Accept GUI action'),
      suggestionFixture('gui-suggestion-reject', 'Reject GUI action'),
      suggestionFixture('gui-suggestion-defer', 'Defer GUI action'),
    ]
    mockInvoke.mockImplementation((cmd: string, payload?: { suggestionId?: string }) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(pendingSuggestions)
      if (cmd === 'submit_suggestion_feedback') {
        pendingSuggestions = pendingSuggestions.filter((item) => item.id !== payload?.suggestionId)
        return Promise.resolve(undefined)
      }
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    await screen.findByText('Accept GUI action')

    fireEvent.click(
      within(screen.getByLabelText('Suggestion: Accept GUI action')).getByRole('button', { name: 'Accept' }),
    )

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
        suggestionId: 'gui-suggestion-accept',
        action: 'accept',
        snoozeMinutes: undefined,
      })
    })
    await waitFor(() => {
      expect(screen.queryByText('Accept GUI action')).not.toBeInTheDocument()
    })

    fireEvent.click(
      within(screen.getByLabelText('Suggestion: Reject GUI action')).getByRole('button', { name: 'Reject' }),
    )

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
        suggestionId: 'gui-suggestion-reject',
        action: 'reject',
        snoozeMinutes: undefined,
      })
    })
    await waitFor(() => {
      expect(screen.queryByText('Reject GUI action')).not.toBeInTheDocument()
    })

    fireEvent.click(
      within(screen.getByLabelText('Suggestion: Defer GUI action')).getByRole('button', { name: 'Later' }),
    )
    fireEvent.click(screen.getByRole('button', { name: '30 minutes' }))

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
        suggestionId: 'gui-suggestion-defer',
        action: 'defer',
        snoozeMinutes: 30,
      })
    })
    await waitFor(() => {
      expect(screen.queryByText('Defer GUI action')).not.toBeInTheDocument()
    })
  })

  it('routes the explain GUI action to chat IPC without removing the active suggestion', async () => {
    const suggestion = suggestionFixture('gui-suggestion-explain', 'Explain GUI action')
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve([suggestion])
      if (cmd === 'explain_suggestion_in_chat') return Promise.resolve('chat-session-1')
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    await screen.findByText('Explain GUI action')

    fireEvent.click(
      within(screen.getByLabelText('Suggestion: Explain GUI action')).getByRole('button', { name: 'Explain' }),
    )

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('explain_suggestion_in_chat', {
        suggestionId: 'gui-suggestion-explain',
      })
    })
    expect(screen.getByText('Explain GUI action')).toBeInTheDocument()
  })

  it('replays a recorded GUI suggestion payload through policy-gated feedback without live model calls', async () => {
    const replay = loadSuggestionReplayFixture()
    let pendingSuggestions = [...replay.suggestions]
    mockInvoke.mockImplementation((cmd: string, payload?: { suggestionId?: string }) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(pendingSuggestions)
      if (cmd === 'submit_suggestion_feedback') {
        pendingSuggestions = pendingSuggestions.filter((item) => item.id !== payload?.suggestionId)
        return Promise.resolve(undefined)
      }
      if (cmd === 'explain_suggestion_in_chat') return Promise.resolve('chat-session-record-replay')
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    for (const title of replay.expected.visible_proposal_titles) {
      await screen.findByText(title)
    }

    const proposal = screen.getByLabelText(`Suggestion: ${replay.expected.visible_proposal_titles[0]}`)
    for (const label of replay.expected.policy_gate_action_labels) {
      expect(within(proposal).getByRole('button', { name: label })).toBeInTheDocument()
    }

    const invokedCommandsBeforeAction = mockInvoke.mock.calls.map(([cmd]) => cmd)
    expect(invokedCommandsBeforeAction).not.toContain('submit_suggestion_feedback')
    expect(invokedCommandsBeforeAction).not.toContain('execute_automation_action')
    expect(invokedCommandsBeforeAction).not.toContain('run_suggestion_action')
    expect(replay.privacy.action_not_auto_executed).toBe(true)

    fireEvent.click(within(proposal).getByRole('button', { name: 'Accept' }))

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
        suggestionId: 'recorded-llm-proposal-policy-gate',
        action: 'accept',
        snoozeMinutes: undefined,
      })
    })

    const feedbackCall = mockInvoke.mock.calls.find(([cmd]) => cmd === 'submit_suggestion_feedback')
    expect(feedbackCall).toBeTruthy()
    const feedbackPayload = feedbackCall?.[1] as Record<string, unknown>
    expect(Object.keys(feedbackPayload).sort()).toEqual(replay.expected.audit_safe_feedback_fields)
    for (const field of replay.privacy.disallowed_feedback_fields) {
      expect(feedbackPayload).not.toHaveProperty(field)
    }
  })

  it('replays Calculator GUI context into a target-matched suggestion without pre-consent GUI mutation', async () => {
    const replay = loadCalculatorSuggestionReplayFixture()
    let pendingSuggestions = [...replay.suggestions]
    mockInvoke.mockImplementation((cmd: string, payload?: { suggestionId?: string }) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(pendingSuggestions)
      if (cmd === 'submit_suggestion_feedback') {
        pendingSuggestions = pendingSuggestions.filter((item) => item.id !== payload?.suggestionId)
        return Promise.resolve(undefined)
      }
      if (cmd === 'explain_suggestion_in_chat') return Promise.resolve('chat-session-calculator-context')
      return Promise.resolve(undefined)
    })

    expect(replay.native_context.active_app).toBe(replay.expected.target_app)
    expect(replay.native_context.active_process.process_name).toBe(replay.expected.target_app)
    expect(replay.native_context.active_process.bundle_id).toBe(replay.native_context.bundle_id)
    expect(replay.native_context.target_entity.label).toBe(replay.expected.target_entity_label)
    expect(replay.native_context.target_entity.value).toBe(replay.expected.target_display_value)
    expect(replay.native_context.highlight.target_entity_id).toBe(replay.native_context.target_entity.entity_id)
    expect(replay.native_context.highlight.bounds).toEqual(replay.native_context.target_entity.bounds)
    expect(
      containsRect(replay.native_context.active_process.window_bounds, replay.native_context.highlight.bounds),
    ).toBe(true)
    expect(replay.native_context.target_entity.center.x).toBeGreaterThan(replay.native_context.highlight.bounds.x)
    expect(replay.native_context.target_entity.center.y).toBeGreaterThan(replay.native_context.highlight.bounds.y)
    expect(replay.native_context.target_entity.sources).toEqual(expect.arrayContaining(['ax', 'ocr', 'screenshot']))

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    for (const title of replay.expected.visible_proposal_titles) {
      await screen.findByText(title)
    }

    const proposal = screen.getByLabelText(`Suggestion: ${replay.expected.visible_proposal_titles[0]}`)
    expect(proposal).toHaveTextContent('Calculator')
    expect(proposal).toHaveTextContent(replay.expected.target_display_value)

    for (const label of replay.expected.policy_gate_action_labels) {
      expect(within(proposal).getByRole('button', { name: label })).toBeInTheDocument()
    }

    const invokedCommandsBeforeAction = mockInvoke.mock.calls.map(([cmd]) => String(cmd))
    for (const command of replay.expected.forbidden_preconsent_commands) {
      expect(invokedCommandsBeforeAction).not.toContain(command)
    }
    expect(replay.privacy.action_not_auto_executed).toBe(true)
    expect(replay.privacy.host_input_used).toBe(false)

    fireEvent.click(within(proposal).getByRole('button', { name: 'Accept' }))

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
        suggestionId: 'calculator-result-explain-proposal',
        action: 'accept',
        snoozeMinutes: undefined,
      })
    })

    const feedbackCall = mockInvoke.mock.calls.find(([cmd]) => cmd === 'submit_suggestion_feedback')
    expect(feedbackCall).toBeTruthy()
    const feedbackPayload = feedbackCall?.[1] as Record<string, unknown>
    expect(Object.keys(feedbackPayload).sort()).toEqual(replay.expected.feedback_payload_fields)
    for (const field of replay.privacy.raw_gui_context_not_in_feedback) {
      expect(feedbackPayload).not.toHaveProperty(field)
    }
  })

  it('renders an unread contour marker at the GUI target and opens an adjacent suggestion popover', async () => {
    const replay = loadCalculatorSuggestionReplayFixture()
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(replay.suggestions)
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestion-surface')).toBe(true)
      expect(listeners.has('overlay:suggestions-changed')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestion-surface')?.({
        payload: {
          placement: 'adjacent-popover',
          anchor: replay.native_context,
        },
      })
      listeners.get('overlay:suggestions-changed')?.({ payload: { count: replay.suggestions.length } })
    })

    const marker = await screen.findByRole('button', { name: /new suggestions for Calculator result 46/i })
    expect(screen.getByTestId('suggestion-contour-marker')).toHaveStyle({
      left: '228px',
      top: '174px',
      width: '184px',
      height: '52px',
    })

    fireEvent.click(marker)

    const panel = await screen.findByLabelText('Suggestions panel')
    expect(panel).toHaveAttribute('data-placement', 'adjacent-popover')
    expect(panel).toHaveTextContent('Calculator > result 46')
    expect(await screen.findByText('Explain the Calculator result')).toBeInTheDocument()
  })

  it('can attach suggestions as a side panel to the active process window', async () => {
    const replay = loadCalculatorSuggestionReplayFixture()
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(replay.suggestions)
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestion-surface')).toBe(true)
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestion-surface')?.({
        payload: {
          placement: 'window-side-panel',
          anchor: replay.native_context,
        },
      })
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    const panel = await screen.findByLabelText('Suggestions panel')
    expect(panel).toHaveAttribute('data-placement', 'window-side-panel')
    expect(panel).toHaveTextContent('Calculator > result 46')
    expect(panel).toHaveStyle({
      left: '472px',
      top: '120px',
    })
  })

  it('keeps anchored suggestion surfaces visible when the target is near the viewport edge', async () => {
    const replay = loadCalculatorSuggestionReplayFixture()
    const edgeAnchor = JSON.parse(JSON.stringify(replay.native_context)) as typeof replay.native_context
    edgeAnchor.active_process.window_bounds = { x: 836, y: 620, width: 180, height: 220 }
    edgeAnchor.highlight.bounds = { x: 948, y: 710, width: 56, height: 42 }
    Object.defineProperty(window, 'innerWidth', { value: 1024, configurable: true })
    Object.defineProperty(window, 'innerHeight', { value: 768, configurable: true })

    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(replay.suggestions)
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestion-surface')).toBe(true)
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestion-surface')?.({
        payload: {
          placement: 'window-side-panel',
          anchor: edgeAnchor,
        },
      })
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    const panel = await screen.findByLabelText('Suggestions panel')
    expect(panel).toHaveAttribute('data-placement', 'window-side-panel')
    let left = Number.parseFloat(panel.style.left)
    let top = Number.parseFloat(panel.style.top)
    const maxHeight = Number.parseFloat(panel.style.maxHeight)
    expect(left).toBeLessThan(edgeAnchor.active_process.window_bounds.x)
    expect(left).toBeGreaterThanOrEqual(16)
    expect(top + maxHeight).toBeLessThanOrEqual(window.innerHeight - 12)

    await act(async () => {
      listeners.get('overlay:set-suggestion-surface')?.({
        payload: {
          placement: 'adjacent-popover',
          anchor: edgeAnchor,
        },
      })
    })

    await waitFor(() => {
      expect(panel).toHaveAttribute('data-placement', 'adjacent-popover')
    })
    left = Number.parseFloat(panel.style.left)
    top = Number.parseFloat(panel.style.top)
    expect(left).toBeLessThan(edgeAnchor.highlight.bounds.x)
    expect(left).toBeGreaterThanOrEqual(16)
    expect(top + 260).toBeLessThanOrEqual(window.innerHeight - 12)
  })

  it('filters GUI suggestions to the active app target across Notes, Finder, and Safari scenarios', async () => {
    const replay = loadMultiAppGuiSuggestionFixture()
    mockInvoke.mockImplementation((cmd: string, payload?: { suggestionId?: string }) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(replay.suggestions)
      if (cmd === 'submit_suggestion_feedback') return Promise.resolve({ suggestionId: payload?.suggestionId })
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestion-surface')).toBe(true)
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    for (const scenario of replay.scenarios) {
      mockInvoke.mockClear()
      await act(async () => {
        listeners.get('overlay:set-suggestion-surface')?.({
          payload: {
            placement: scenario.placement,
            anchor: scenario.native_context,
          },
        })
        listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
      })

      const surface =
        scenario.placement === 'bottom-dock'
          ? await screen.findByTestId('suggestion-bottom-dock')
          : await screen.findByLabelText('Suggestions panel')

      await within(surface).findByText(scenario.expected.visible_title)
      expect(surface).toHaveTextContent(scenario.expected.breadcrumb)
      for (const hiddenTitle of scenario.expected.hidden_titles) {
        expect(within(surface).queryByText(hiddenTitle)).not.toBeInTheDocument()
      }

      const invokedCommandsBeforeAction = mockInvoke.mock.calls.map(([cmd]) => String(cmd))
      for (const command of scenario.expected.forbidden_preconsent_commands) {
        expect(invokedCommandsBeforeAction).not.toContain(command)
      }

      fireEvent.click(within(surface).getByRole('button', { name: 'Accept' }))
      await waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
          suggestionId: scenario.expected.suggestion_id,
          action: 'accept',
          snoozeMinutes: undefined,
        })
      })
    }
  })

  it('prefers target-scoped GUI suggestions over generic suggestions in the same Safari window', async () => {
    const replay = loadTargetScopedGuiSuggestionPriorityFixture()
    mockInvoke.mockImplementation((cmd: string, payload?: { suggestionId?: string }) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(replay.suggestions)
      if (cmd === 'submit_suggestion_feedback') return Promise.resolve({ suggestionId: payload?.suggestionId })
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestion-surface')).toBe(true)
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    for (const scenario of replay.scenarios) {
      mockInvoke.mockClear()
      await act(async () => {
        listeners.get('overlay:set-suggestion-surface')?.({
          payload: {
            placement: scenario.placement,
            anchor: scenario.native_context,
          },
        })
        listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
      })

      const surface =
        scenario.placement === 'bottom-dock'
          ? await screen.findByTestId('suggestion-bottom-dock')
          : await screen.findByLabelText('Suggestions panel')

      await within(surface).findByText(scenario.expected.visible_title)
      expect(surface).toHaveTextContent(scenario.expected.breadcrumb)
      for (const hiddenTitle of scenario.expected.hidden_titles) {
        expect(within(surface).queryByText(hiddenTitle)).not.toBeInTheDocument()
      }

      fireEvent.click(within(surface).getByRole('button', { name: 'Accept' }))
      await waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
          suggestionId: scenario.expected.suggestion_id,
          action: 'accept',
          snoozeMinutes: undefined,
        })
      })
    }
  })

  it('runs the full GUI suggestion workflow from captured target to audit-safe accept feedback', async () => {
    const replay = loadEndToEndSuggestionWorkflowFixture()
    let pendingSuggestions = [...replay.suggestions]
    mockInvoke.mockImplementation((cmd: string, payload?: { suggestionId?: string }) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(pendingSuggestions)
      if (cmd === 'submit_suggestion_feedback') {
        pendingSuggestions = pendingSuggestions.filter((item) => item.id !== payload?.suggestionId)
        return Promise.resolve({ auditTraceId: 'audit-e2e-workflow-001' })
      }
      return Promise.resolve(undefined)
    })

    expect(replay.workflow.steps).toEqual([
      'capture_context',
      'resolve_process_gui_anchor',
      'render_contour_marker',
      'open_attached_suggestion_surface',
      'accept_policy_gated_proposal',
      'write_audit_safe_feedback',
    ])
    expect(replay.workflow.capture_inputs).toEqual(
      expect.arrayContaining(['window_bounds', 'screenshot_crop', 'ax_tree', 'ocr_text']),
    )
    expect(replay.workflow.privacy_gates).toEqual({
      live_provider_called: false,
      action_not_auto_executed: true,
      confirmation_required: true,
    })
    expect(replay.privacy.host_input_used).toBe(false)
    expect(replay.privacy.host_mutation).toBe(false)
    expect(replay.privacy.guest_mutation).toBe(false)

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestion-surface')).toBe(true)
      expect(listeners.has('overlay:suggestions-changed')).toBe(true)
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestion-surface')?.({
        payload: {
          placement: 'adjacent-popover',
          anchor: replay.native_context,
        },
      })
      listeners.get('overlay:suggestions-changed')?.({ payload: { count: replay.suggestions.length } })
    })

    const marker = await screen.findByRole('button', { name: replay.expected.contour_marker_label })
    expect(screen.getByTestId('suggestion-contour-marker')).toHaveStyle({
      left: `${replay.native_context.highlight.bounds.x}px`,
      top: `${replay.native_context.highlight.bounds.y}px`,
      width: `${replay.native_context.highlight.bounds.width}px`,
      height: `${replay.native_context.highlight.bounds.height}px`,
    })
    fireEvent.click(marker)

    await act(async () => {
      listeners.get('overlay:set-suggestion-surface')?.({
        payload: {
          placement: replay.expected.side_panel_placement,
          anchor: replay.native_context,
        },
      })
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    const panel = await screen.findByLabelText('Suggestions panel')
    expect(panel).toHaveAttribute('data-placement', replay.expected.side_panel_placement)
    expect(panel).toHaveTextContent(replay.expected.breadcrumb)

    const proposal = await within(panel).findByLabelText(`Suggestion: ${replay.expected.proposal_title}`)
    expect(proposal).toHaveTextContent(replay.expected.policy_gate_label)

    const invokedCommandsBeforeAction = mockInvoke.mock.calls.map(([cmd]) => String(cmd))
    for (const command of replay.expected.forbidden_preconsent_commands) {
      expect(invokedCommandsBeforeAction).not.toContain(command)
    }

    fireEvent.click(within(proposal).getByRole('button', { name: 'Accept' }))

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
        suggestionId: 'calculator-result-e2e-workflow-proposal',
        action: 'accept',
        snoozeMinutes: undefined,
      })
    })
    await screen.findByText(replay.expected.success_toast)

    const feedbackCall = mockInvoke.mock.calls.find(([cmd]) => cmd === 'submit_suggestion_feedback')
    expect(feedbackCall).toBeTruthy()
    const feedbackPayload = feedbackCall?.[1] as Record<string, unknown>
    expect(Object.keys(feedbackPayload).sort()).toEqual(replay.expected.feedback_payload_fields)
    for (const field of replay.privacy.raw_context_not_in_feedback) {
      expect(feedbackPayload).not.toHaveProperty(field)
    }
    expect(JSON.stringify(feedbackPayload)).not.toContain(replay.native_context.display_value)
    await waitFor(() => {
      expect(within(panel).queryByText(replay.expected.proposal_title)).not.toBeInTheDocument()
    })
  })

  it('renders a metadata-only RUM replay trail for the GUI suggestion workflow', async () => {
    const replay = loadSuggestionRumReplayTrailFixture()
    let pendingSuggestions = [...replay.suggestions]
    mockInvoke.mockImplementation((cmd: string, payload?: { suggestionId?: string }) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(pendingSuggestions)
      if (cmd === 'record_suggestion_replay_event') {
        return Promise.resolve({ traceId: `rum-${payload?.suggestionId ?? 'target'}` })
      }
      if (cmd === 'submit_suggestion_feedback') {
        pendingSuggestions = pendingSuggestions.filter((item) => item.id !== payload?.suggestionId)
        return Promise.resolve({ auditTraceId: 'audit-rum-replay-001' })
      }
      return Promise.resolve(undefined)
    })

    expect(replay.workflow.steps).toEqual([
      'render_contour_marker',
      'open_attached_suggestion_surface',
      'show_replay_trail',
      'record_metadata_only_rum',
      'accept_policy_gated_proposal',
    ])
    expect(replay.workflow.rum_phases).toEqual(['marker_opened', 'proposal_visible', 'feedback_submitted'])
    expect(replay.workflow.privacy_gates).toEqual({
      metadata_only: true,
      raw_context_included: false,
      action_not_auto_executed: true,
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestion-surface')).toBe(true)
      expect(listeners.has('overlay:suggestions-changed')).toBe(true)
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestion-surface')?.({
        payload: {
          placement: 'adjacent-popover',
          anchor: replay.native_context,
        },
      })
      listeners.get('overlay:suggestions-changed')?.({ payload: { count: replay.suggestions.length } })
    })

    fireEvent.click(await screen.findByRole('button', { name: replay.expected.marker_label }))

    await act(async () => {
      listeners.get('overlay:set-suggestion-surface')?.({
        payload: {
          placement: 'window-side-panel',
          anchor: replay.native_context,
        },
      })
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    const panel = await screen.findByLabelText('Suggestions panel')
    expect(panel).toHaveTextContent(replay.expected.breadcrumb)

    const replayTrail = await within(panel).findByTestId('suggestion-replay-trail')
    expect(replayTrail).toHaveAttribute('aria-label', replay.expected.replay_trail_label)
    for (const label of replay.expected.replay_step_labels) {
      expect(replayTrail).toHaveTextContent(label)
    }

    const proposal = await within(panel).findByLabelText(`Suggestion: ${replay.expected.proposal_title}`)
    const commandsBeforeAction = mockInvoke.mock.calls.map(([cmd]) => String(cmd))
    for (const command of replay.expected.forbidden_preconsent_commands) {
      expect(commandsBeforeAction).not.toContain(command)
    }

    fireEvent.click(within(proposal).getByRole('button', { name: 'Accept' }))

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
        suggestionId: 'calculator-result-rum-replay-proposal',
        action: 'accept',
        snoozeMinutes: undefined,
      })
    })

    const replayCalls = mockInvoke.mock.calls.filter(([cmd]) => cmd === 'record_suggestion_replay_event')
    expect(replayCalls.length).toBeGreaterThanOrEqual(3)
    expect(replayCalls.map(([, payload]) => (payload as { payload: { phase: string } }).payload.phase)).toEqual(
      expect.arrayContaining(replay.workflow.rum_phases),
    )

    for (const [, payload] of replayCalls) {
      const eventPayload = (payload as { payload: Record<string, unknown> }).payload
      expect(Object.keys(eventPayload).sort()).toEqual(replay.expected.replay_payload_fields)
      expect(eventPayload.rawContextIncluded).toBe(false)
      const serialized = JSON.stringify(eventPayload)
      for (const fragment of replay.expected.forbidden_replay_fragments) {
        expect(serialized).not.toContain(fragment)
      }
    }
  })

  it('can show suggestions in a bottom dock with a target breadcrumb', async () => {
    const replay = loadCalculatorSuggestionReplayFixture()
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(replay.suggestions)
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestion-surface')).toBe(true)
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestion-surface')?.({
        payload: {
          placement: 'bottom-dock',
          anchor: replay.native_context,
        },
      })
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    const dock = await screen.findByTestId('suggestion-bottom-dock')
    expect(dock).toHaveTextContent('Calculator > result 46')
    expect(dock).toHaveTextContent('No auto action')
    expect(within(dock).getByRole('button', { name: 'Accept' })).toBeInTheDocument()
    expect(within(dock).getByRole('button', { name: 'Explain' })).toBeInTheDocument()
    expect(within(dock).getByRole('button', { name: 'Later' })).toBeInTheDocument()
  })

  it('shows clarification instead of action affordances when Calculator context mismatches the instruction target', async () => {
    const replay = loadCalculatorClarificationFixture()
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(replay.suggestions)
      if (cmd === 'explain_suggestion_in_chat') return Promise.resolve('chat-session-calculator-clarification')
      return Promise.resolve(undefined)
    })

    expect(replay.native_context.active_app).toBe(replay.expected.visible_target_app)
    expect(replay.instruction).toContain('note')

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    await screen.findByText(replay.expected.clarification_title)
    const clarification = screen.getByLabelText(`Suggestion: ${replay.expected.clarification_title}`)
    expect(clarification).toHaveTextContent('Calculator')
    expect(clarification).toHaveTextContent('note')

    for (const label of replay.expected.forbidden_action_labels) {
      expect(within(clarification).queryByRole('button', { name: label })).not.toBeInTheDocument()
    }
    for (const label of replay.expected.allowed_labels) {
      expect(within(clarification).getByRole('button', { name: label })).toBeInTheDocument()
    }

    const invokedCommandsBeforeClarify = mockInvoke.mock.calls.map(([cmd]) => String(cmd))
    for (const command of replay.expected.forbidden_preconsent_commands) {
      expect(invokedCommandsBeforeClarify).not.toContain(command)
    }

    fireEvent.click(within(clarification).getByRole('button', { name: 'Explain' }))

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('explain_suggestion_in_chat', {
        suggestionId: 'calculator-note-target-clarification',
      })
    })
  })

  it('falls back quietly when the suggestion provider is unavailable and retries without leaking raw context', async () => {
    const replay = loadProviderUnavailableFallbackFixture()
    let getPendingCalls = 0
    let resolveRetry: (() => void) | null = null
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})

    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') {
        getPendingCalls += 1
        if (getPendingCalls === 1) {
          return Promise.reject(new Error(replay.provider_failure.message))
        }
        return new Promise<SuggestionViewDto[]>((resolve) => {
          resolveRetry = () => resolve(replay.local_fallback_suggestions)
        })
      }
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    await screen.findByText(replay.expected.generic_error_message)
    for (const fragment of replay.provider_failure.raw_screen_context_fragments) {
      expect(screen.queryByText(fragment, { exact: false })).not.toBeInTheDocument()
    }

    const retryButton = screen.getByRole('button', { name: replay.expected.retry_label })
    fireEvent.click(retryButton)
    await waitFor(() => {
      expect(screen.getByRole('button', { name: replay.expected.retry_pending_label })).toBeDisabled()
    })
    fireEvent.click(screen.getByRole('button', { name: replay.expected.retry_pending_label }))
    expect(getPendingCalls).toBe(2)

    const commandsBeforeProviderRecovery = mockInvoke.mock.calls.map(([cmd]) => String(cmd))
    for (const command of replay.expected.forbidden_provider_commands) {
      expect(commandsBeforeProviderRecovery).not.toContain(command)
    }

    await act(async () => {
      resolveRetry?.()
    })

    await screen.findByText(replay.expected.fallback_title)
    expect(screen.queryByText(replay.expected.generic_error_message)).not.toBeInTheDocument()

    const warningText = warnSpy.mock.calls.flat().join(' ')
    for (const fragment of replay.provider_failure.raw_screen_context_fragments) {
      expect(warningText).not.toContain(fragment)
    }
    warnSpy.mockRestore()
  })

  it('keeps suggestion panel loading, empty, dismissed, error, and recovered states stable', async () => {
    const replay = loadSuggestionPanelStateFixture()
    let getPendingCalls = 0
    let resolveInitial: (() => void) | null = null
    let resolveRecovery: (() => void) | null = null
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})

    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') {
        getPendingCalls += 1
        if (getPendingCalls === 1) {
          return new Promise<SuggestionViewDto[]>((resolve) => {
            resolveInitial = () => resolve(replay.initial_suggestions)
          })
        }
        if (getPendingCalls === 2) {
          return Promise.resolve(replay.initial_suggestions)
        }
        if (getPendingCalls === 3) {
          return Promise.reject(new Error(replay.provider_failure.message))
        }
        return new Promise<SuggestionViewDto[]>((resolve) => {
          resolveRecovery = () => resolve(replay.recovered_suggestions)
        })
      }
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
      expect(listeners.has('overlay:suggestions-changed')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    const panel = screen.getByLabelText('Suggestions panel')
    expect(panel).toHaveClass(replay.expected.open_panel_class)
    await screen.findByText(replay.expected.loading_message)

    await act(async () => {
      resolveInitial?.()
    })

    await screen.findByText(replay.expected.empty_message)

    fireEvent.click(screen.getByRole('button', { name: 'Close suggestions panel' }))
    await waitFor(() => {
      expect(panel).toHaveClass(replay.expected.closed_panel_class)
    })

    await act(async () => {
      listeners.get('overlay:suggestions-changed')?.({ payload: { count: 1 } })
    })
    expect(panel).toHaveClass(replay.expected.closed_panel_class)

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })
    await screen.findByText(replay.expected.generic_error_message)
    expect(screen.queryByText(replay.expected.raw_context_fragment, { exact: false })).not.toBeInTheDocument()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: replay.expected.retry_label }))
    })
    await waitFor(() => {
      expect(resolveRecovery).toBeTruthy()
    })
    await act(async () => {
      resolveRecovery?.()
    })

    await screen.findByText(replay.expected.recovered_title)
    expect(screen.queryByText(replay.expected.generic_error_message)).not.toBeInTheDocument()
    expect(getPendingCalls).toBe(4)

    const warningText = warnSpy.mock.calls.flat().join(' ')
    expect(warningText).not.toContain(replay.expected.raw_context_fragment)
    warnSpy.mockRestore()
  })

  it('redacts sensitive GUI text before rendering suggestions or feedback payloads', async () => {
    const replay = loadSensitiveGuiRedactionFixture()
    let pendingSuggestions = [...replay.suggestions]
    mockInvoke.mockImplementation((cmd: string, payload?: { suggestionId?: string }) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(pendingSuggestions)
      if (cmd === 'submit_suggestion_feedback') {
        pendingSuggestions = pendingSuggestions.filter((item) => item.id !== payload?.suggestionId)
        return Promise.resolve(undefined)
      }
      return Promise.resolve(undefined)
    })

    expect(replay.privacy.prompt_redacted_before_display).toBe(true)
    expect(replay.privacy.audit_metadata_only.raw_value_persisted).toBe(false)
    expect(replay.privacy.audit_metadata_only.redaction_classes).toEqual(replay.gui_context.redaction_classes)

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    const proposal = await screen.findByLabelText(`Suggestion: ${replay.expected.redacted_title}`)
    for (const fragment of replay.expected.visible_redacted_fragments) {
      expect(proposal).toHaveTextContent(fragment)
    }
    for (const fragment of replay.gui_context.raw_sensitive_fragments) {
      expect(screen.queryByText(fragment, { exact: false })).not.toBeInTheDocument()
      expect(proposal.getAttribute('aria-label')).not.toContain(fragment)
    }

    fireEvent.click(within(proposal).getByRole('button', { name: 'Accept' }))

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
        suggestionId: 'notes-sensitive-redaction-proposal',
        action: 'accept',
        snoozeMinutes: undefined,
      })
    })

    const feedbackCall = mockInvoke.mock.calls.find(([cmd]) => cmd === 'submit_suggestion_feedback')
    expect(feedbackCall).toBeTruthy()
    const feedbackPayload = feedbackCall?.[1] as Record<string, unknown>
    expect(Object.keys(feedbackPayload).sort()).toEqual(replay.expected.feedback_payload_fields)
    for (const field of replay.expected.forbidden_feedback_fields) {
      expect(feedbackPayload).not.toHaveProperty(field)
    }
    const feedbackText = JSON.stringify(feedbackPayload)
    for (const fragment of replay.gui_context.raw_sensitive_fragments) {
      expect(feedbackText).not.toContain(fragment)
    }
  })

  it('supports keyboard-only suggestion review, feedback, snooze, explain, and dismissal', async () => {
    const replay = loadKeyboardA11yFlowFixture()
    const user = userEvent.setup()
    let pendingSuggestions = [...replay.suggestions]
    mockInvoke.mockImplementation((cmd: string, payload?: { suggestionId?: string }) => {
      if (cmd === 'get_capture_status') return Promise.resolve({ paused: false, indicator_visible: true })
      if (cmd === 'get_pending_suggestions') return Promise.resolve(pendingSuggestions)
      if (cmd === 'submit_suggestion_feedback') {
        pendingSuggestions = pendingSuggestions.filter((item) => item.id !== payload?.suggestionId)
        return Promise.resolve(undefined)
      }
      if (cmd === 'explain_suggestion_in_chat') return Promise.resolve('chat-session-keyboard-a11y')
      return Promise.resolve(undefined)
    })

    render(
      <I18nextProvider i18n={i18n}>
        <OverlayApp />
      </I18nextProvider>,
    )

    await waitFor(() => {
      expect(listeners.has('overlay:set-suggestions-panel')).toBe(true)
    })

    await act(async () => {
      listeners.get('overlay:set-suggestions-panel')?.({ payload: { open: true } })
    })

    const panel = screen.getByLabelText(replay.expected.panel_label)
    for (const item of replay.suggestions) {
      await screen.findByText(item.title)
    }

    for (const action of replay.expected.actions) {
      const proposal = screen.getByLabelText(`Suggestion: ${action.title}`)
      const button = within(proposal).getByRole('button', { name: action.button })
      await focusByTab(user, button)
      expect(document.activeElement).toBe(button)
      await user.keyboard('{Enter}')

      if (action.action === 'defer') {
        const snoozeButton = await screen.findByRole('button', { name: '30 minutes' })
        await focusByTab(user, snoozeButton)
        await user.keyboard('{Enter}')
      }

      if (action.command === 'explain_suggestion_in_chat') {
        await waitFor(() => {
          expect(mockInvoke).toHaveBeenCalledWith('explain_suggestion_in_chat', {
            suggestionId: action.suggestion_id,
          })
        })
        continue
      }

      await waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith('submit_suggestion_feedback', {
          suggestionId: action.suggestion_id,
          action: action.action,
          snoozeMinutes: action.snooze_minutes,
        })
      })
    }

    await user.keyboard('{Escape}')
    await waitFor(() => {
      expect(panel).toHaveClass(replay.expected.closed_panel_class)
    })
  }, 10_000)
})
