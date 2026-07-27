import { describe, expect, it } from 'vitest'
import type { RegimeOverride, UserOverrideAction } from '../../api/contracts'
import { getOverrideEffectLabel } from './OverridesSection'

function overrideWith(user_action: UserOverrideAction): RegimeOverride {
  return {
    override_id: 'override-test',
    segment_id: 'segment-test',
    original_regime_id: 'focus',
    user_action,
    created_at: '2026-07-12T00:00:00Z',
  }
}

describe('getOverrideEffectLabel', () => {
  const t = (key: string) => key

  it.each([
    [{ type: 'MARK_AS_NOISE' } as const, 'recalibration.effectMarkAsNoise'],
    [{ type: 'REASSIGN_REGIME', target_regime_id: 'deep-work' } as const, 'recalibration.effectReassignRegime'],
    [
      { type: 'MARK_AS_PERSONAL_TIME', from: '2026-07-12T00:00:00Z', to: '2026-07-12T01:00:00Z' } as const,
      'recalibration.effectPersonalTime',
    ],
  ])('maps %o to %s', (action, expected) => {
    expect(getOverrideEffectLabel(overrideWith(action), t)).toBe(expected)
  })
})
