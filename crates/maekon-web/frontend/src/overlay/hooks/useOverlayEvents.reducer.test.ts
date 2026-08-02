/**
 * Overlay reducer unit tests (#9637).
 *
 * The reducer is the overlay's single state SSOT (CLAUDE.md guardrail: "All
 * overlay state flows through the useOverlayEvents reducer... Missing any one
 * causes silent failures"), yet 13 of its 23 branches had no automated
 * coverage of any kind — App.test.tsx only ever drove 4 of the 17 registered
 * Tauri events. These tests exercise every branch directly.
 */

import { describe, expect, it } from 'vitest'
import type { CoachingPayload, SuggestionViewDto } from '../types'
import { initialState, reducer } from './useOverlayEvents'

function coaching(id: string): CoachingPayload {
  return { message_id: id, text: `msg-${id}` } as CoachingPayload
}

function suggestion(id: string): SuggestionViewDto {
  return { id, title: `s-${id}` } as SuggestionViewDto
}

describe('overlay reducer (#9637)', () => {
  it('show-coaching displays the first message and queues subsequent ones', () => {
    const first = reducer(initialState, { type: 'show-coaching', payload: coaching('a') })
    expect(first.coaching?.message_id).toBe('a')
    expect(first.coachingQueue).toHaveLength(0)

    const second = reducer(first, { type: 'show-coaching', payload: coaching('b') })
    expect(second.coaching?.message_id).toBe('a')
    expect(second.coachingQueue.map((c) => c.message_id)).toEqual(['b'])
  })

  it('upgrade-message personalizes only the currently-shown message', () => {
    const shown = reducer(initialState, { type: 'show-coaching', payload: coaching('a') })
    const upgraded = reducer(shown, {
      type: 'upgrade-message',
      payload: { message_id: 'a', personalized_text: 'better' },
    })
    expect(upgraded.coaching?.text).toBe('better')

    const mismatch = reducer(shown, {
      type: 'upgrade-message',
      payload: { message_id: 'other', personalized_text: 'nope' },
    })
    expect(mismatch).toBe(shown)
  })

  it('dismiss promotes the next queued coaching message, then clears', () => {
    let state = reducer(initialState, { type: 'show-coaching', payload: coaching('a') })
    state = reducer(state, { type: 'show-coaching', payload: coaching('b') })

    state = reducer(state, { type: 'dismiss' })
    expect(state.coaching?.message_id).toBe('b')
    expect(state.coachingQueue).toHaveLength(0)

    state = reducer(state, { type: 'dismiss' })
    expect(state.coaching).toBeNull()
  })

  it('update-focus sets the highlight but never fights an active detection scene', () => {
    const highlight = { element_id: 'el-1' } as never
    const focused = reducer(initialState, { type: 'update-focus', payload: highlight })
    expect(focused.focusHighlight).toBe(highlight)

    const withScene = reducer(initialState, {
      type: 'detection-update',
      payload: { scene_id: 'scene' } as never,
    })
    const ignored = reducer(withScene, { type: 'update-focus', payload: highlight })
    expect(ignored.focusHighlight).toBeNull()

    expect(reducer(focused, { type: 'clear-focus' }).focusHighlight).toBeNull()
  })

  it('update-goals / set-mode / set-focus-mode / capture-state-changed store their payloads', () => {
    const goals = [{ label: 'g' }] as never
    expect(reducer(initialState, { type: 'update-goals', payload: goals }).goals).toBe(goals)
    expect(reducer(initialState, { type: 'set-mode', payload: 'expanded' as never }).mode).toBe('expanded')

    const fm = reducer(initialState, { type: 'set-focus-mode', payload: { active: true, auto: true } })
    expect(fm.focusMode).toBe(true)
    expect(fm.focusModeAuto).toBe(true)

    const cap = { paused: false, indicator_visible: true, consent_granted: true, permitted: true }
    expect(reducer(initialState, { type: 'capture-state-changed', payload: cap }).captureState).toEqual(cap)
  })

  it('suggestion badge counts unseen arrivals and resets when the panel opens', () => {
    // Panel closed: new suggestions increment the badge by the delta.
    let state = reducer(initialState, { type: 'set-suggestions', payload: [suggestion('1'), suggestion('2')] })
    expect(state.suggestionBadgeCount).toBe(2)

    // Re-set with one MORE suggestion → +1, not +3.
    state = reducer(state, {
      type: 'set-suggestions',
      payload: [suggestion('1'), suggestion('2'), suggestion('3')],
    })
    expect(state.suggestionBadgeCount).toBe(3)

    // Opening the panel clears the badge; setting suggestions while open keeps it 0.
    state = reducer(state, { type: 'toggle-suggestions-panel', payload: true })
    expect(state.suggestionsPanelOpen).toBe(true)
    expect(state.suggestionBadgeCount).toBe(0)
    state = reducer(state, { type: 'set-suggestions', payload: [suggestion('4')] })
    expect(state.suggestionBadgeCount).toBe(0)

    // Toggling with no payload flips the panel.
    expect(reducer(state, { type: 'toggle-suggestions-panel' }).suggestionsPanelOpen).toBe(false)
  })

  it('remove-suggestion drops exactly the matching id', () => {
    const seeded = reducer(initialState, {
      type: 'set-suggestions',
      payload: [suggestion('1'), suggestion('2')],
    })
    const removed = reducer(seeded, { type: 'remove-suggestion', payload: '1' })
    expect(removed.suggestions.map((s) => s.id)).toEqual(['2'])
  })

  it('set-suggestion-surface and capture-feedback store their payloads', () => {
    const surface = { placement: 'cursor', anchor: { x: 1, y: 2 } } as never
    expect(reducer(initialState, { type: 'set-suggestion-surface', payload: surface }).suggestionSurface).toBe(surface)
    expect(reducer(initialState, { type: 'capture-feedback', payload: 'ts-1' }).captureFlashTimestamp).toBe('ts-1')
  })

  it('pointer-context-update stores enabled payloads and clears disabled ones', () => {
    const enabled = { enabled: true, x: 5 } as never
    expect(reducer(initialState, { type: 'pointer-context-update', payload: enabled }).pointerContext).toBe(enabled)
    const cleared = reducer(
      { ...initialState, pointerContext: enabled },
      { type: 'pointer-context-update', payload: { enabled: false } as never },
    )
    expect(cleared.pointerContext).toBeNull()
  })

  it('detection lifecycle: update resets selection and focus, select and clear behave', () => {
    const withFocus = reducer(initialState, { type: 'update-focus', payload: { element_id: 'f' } as never })
    const scene = reducer(withFocus, { type: 'detection-update', payload: { scene_id: 's1' } as never })
    expect(scene.detectionScene).not.toBeNull()
    expect(scene.detectionSelectedId).toBeNull()
    expect(scene.focusHighlight).toBeNull()

    const selected = reducer(scene, { type: 'detection-select', payload: 'el-9' })
    expect(selected.detectionSelectedId).toBe('el-9')

    const cleared = reducer(selected, { type: 'detection-clear' })
    expect(cleared.detectionScene).toBeNull()
    expect(cleared.detectionSelectedId).toBeNull()
  })

  it('automation confirmation and codex approval request/dismiss pairs', () => {
    const confirm = { request_id: 'r1' } as never
    const withConfirm = reducer(initialState, { type: 'automation-confirm-request', payload: confirm })
    expect(withConfirm.pendingConfirmation).toBe(confirm)
    expect(reducer(withConfirm, { type: 'automation-confirm-dismiss' }).pendingConfirmation).toBeNull()

    const approval = { approval_id: 'c1' } as never
    const withApproval = reducer(initialState, { type: 'codex-approval-request', payload: approval })
    expect(withApproval.pendingCodexApproval).toBe(approval)
    expect(reducer(withApproval, { type: 'codex-approval-dismiss' }).pendingCodexApproval).toBeNull()
  })

  it('set-fullscreen-policy stores the payload', () => {
    const policy = { hide_on_fullscreen: true } as never
    expect(reducer(initialState, { type: 'set-fullscreen-policy', payload: policy }).fullscreenPolicy).toBe(policy)
  })
})
