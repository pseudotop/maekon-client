import { describe, expect, it } from 'vitest'
import { decodeExactIntentLookup, decodeIntentProvenance, isOpaqueIntentId } from '../intentExecution'
import {
  EXECUTION_B,
  makeAttempt,
  makeFallback,
  makeProvenance,
  opaqueId,
  PRIVATE_CANARY,
  SUBMISSION_A,
  SUBMISSION_B,
} from './fixtures/intentExecution'

describe('bounded intent observation decoder', () => {
  it('preserves a matching response, with an independent copied snapshot', () => {
    const raw = makeFallback()
    const result = decodeIntentProvenance(raw, SUBMISSION_A)
    expect(result).toEqual(raw)
    raw.attempts[0].capabilities.skills_effective = 'used'
    raw.events.reverse()
    expect(result?.attempts[0].capabilities.skills_effective).toBe('unknown')
    expect(result?.events[0].sequence).toBe(0)
  })

  it.each([undefined, null, {}, [], PRIVATE_CANARY])('keeps legacy or malformed observations unknown: %s', (raw) => {
    expect(decodeIntentProvenance(raw, SUBMISSION_A)).toBeNull()
  })

  it('requires exact submission identity even when command and session are identical', () => {
    const a = { command_id: 'same-command', session_id: 'same-session', execution_provenance: makeProvenance() }
    const b = { ...a, execution_provenance: makeProvenance({ execution_id: EXECUTION_B, submission_id: SUBMISSION_B }) }
    expect(decodeExactIntentLookup([a], SUBMISSION_A)?.execution_id).not.toBe(EXECUTION_B)
    expect(decodeExactIntentLookup([b], SUBMISSION_A)).toBeNull()
    expect(decodeExactIntentLookup([a, b], SUBMISSION_A)).toBeNull()
    expect(decodeExactIntentLookup([a, a], SUBMISSION_A)).toBeNull()
    expect(decodeExactIntentLookup([], SUBMISSION_A)).toBeNull()
    expect(decodeExactIntentLookup([b], SUBMISSION_B)?.execution_id).toBe(EXECUTION_B)
  })

  it.each([
    '00000000-0000-0000-0000-000000000000',
    '00000000-0000-4000-8000-00000000000A',
    'a'.repeat(32),
    PRIVATE_CANARY,
  ])('rejects noncanonical correlation %s', (id) => {
    expect(isOpaqueIntentId(id)).toBe(false)
    expect(decodeIntentProvenance(makeProvenance({ submission_id: id }), id)).toBeNull()
  })

  it('orders late events by execution and sequence, accepting only identical duplicates', () => {
    const p = makeProvenance()
    const events = p.events
    const shuffled = { ...p, events: [...events].reverse().concat(events[0]) }
    expect(decodeIntentProvenance(shuffled, SUBMISSION_A)?.events).toEqual(events)
    expect(
      decodeIntentProvenance({ ...p, events: [...events, { ...events[0], event_id: opaqueId(999) }] }, SUBMISSION_A),
    ).toBeNull()
    expect(
      decodeIntentProvenance({ ...p, events: [...events, { ...events[0], sequence: 30 }] }, SUBMISSION_A),
    ).toBeNull()
    expect(
      decodeIntentProvenance(
        { ...p, events: [{ ...events[0], execution_id: EXECUTION_B }, ...events.slice(1)] },
        SUBMISSION_A,
      ),
    ).toBeNull()
  })

  it('keeps gaps, missing boundaries and incomplete observations uncertain', () => {
    const p = makeProvenance()
    for (const events of [[], p.events.slice(1), p.events.slice(0, -1), p.events.filter((e) => e.sequence !== 2)]) {
      const decoded = decodeIntentProvenance({ ...p, events }, SUBMISSION_A)
      expect(decoded?.observation).toBe('partial')
      expect(decoded?.observation_reason).toBe('observation_unavailable')
    }
    expect(decodeIntentProvenance({ ...p, observation: 'partial' }, SUBMISSION_A)?.observation).toBe('partial')
    expect(decodeIntentProvenance({ ...p, observation: 'legacy_unknown' }, SUBMISSION_A)?.observation).toBe(
      'legacy_unknown',
    )
    const wrongTerminal = { ...p, action_outcome: 'denied' }
    expect(decodeIntentProvenance(wrongTerminal, SUBMISSION_A)?.observation).toBe('partial')
  })

  it('enforces attempt linkage and exact attempt/event caps', () => {
    const attempts = Array.from({ length: 8 }, (_, i) =>
      makeAttempt({ attempt_id: opaqueId(30 + i), predecessor: i ? opaqueId(29 + i) : null }),
    )
    const p = makeProvenance({ attempts })
    expect(decodeIntentProvenance(p, SUBMISSION_A)).not.toBeNull()
    expect(
      decodeIntentProvenance(
        { ...p, attempts: [...attempts, makeAttempt({ attempt_id: opaqueId(90) })] },
        SUBMISSION_A,
      ),
    ).toBeNull()
    expect(decodeIntentProvenance(makeProvenance({ attempts: [attempts[1]] }), SUBMISSION_A)).toBeNull()
    expect(decodeIntentProvenance(makeProvenance({ attempts: [attempts[0], attempts[0]] }), SUBMISSION_A)).toBeNull()
    const events = Array.from({ length: 32 }, (_, i) => ({
      ...p.events[0],
      event_id: opaqueId(200 + i),
      sequence: i,
      kind:
        i === 0
          ? p.events[0].kind
          : i === 31
            ? p.events[p.events.length - 1].kind
            : { stage: 'action_started' as const },
    }))
    expect(decodeIntentProvenance({ ...p, events }, SUBMISSION_A)?.events).toHaveLength(32)
    expect(decodeIntentProvenance({ ...p, events: [...events, events[0]] }, SUBMISSION_A)).toBeNull()
    for (const sequence of [-1, 0.5, 32, NaN]) {
      expect(decodeIntentProvenance({ ...p, events: [{ ...events[0], sequence }] }, SUBMISSION_A)).toBeNull()
    }
  })

  it('rejects forbidden fields at every nested wire boundary, and unsafe enum/timestamp values', () => {
    const boundaries = [
      (p: any) => p,
      (p: any) => p.attempts[0],
      (p: any) => p.attempts[0].capabilities,
      (p: any) => p.events[0],
      (p: any) => p.events[0].kind,
    ]
    for (const boundary of boundaries) {
      const p = makeProvenance()
      boundary(p).raw_error = PRIVATE_CANARY
      expect(decodeIntentProvenance(p, SUBMISSION_A)).toBeNull()
    }
    for (const field of [
      'schema_version',
      'storage',
      'requested_provider_class',
      'planning_outcome',
      'action_outcome',
      'observation',
      'observation_reason',
      'config_revision',
      'configuration_reason',
    ]) {
      expect(decodeIntentProvenance({ ...makeProvenance(), [field]: PRIVATE_CANARY }, SUBMISSION_A)).toBeNull()
    }
    for (const field of ['outcome', 'reason', 'effective_provider_class', 'model_revision']) {
      const p = makeProvenance()
      Object.assign(p.attempts[0], { [field]: PRIVATE_CANARY })
      expect(decodeIntentProvenance(p, SUBMISSION_A)).toBeNull()
    }
    for (const field of ['skills_requested', 'skills_context_forwarded', 'skill_context_kind', 'skills_effective']) {
      const p = makeProvenance()
      Object.assign(p.attempts[0].capabilities, { [field]: PRIVATE_CANARY })
      expect(decodeIntentProvenance(p, SUBMISSION_A)).toBeNull()
    }
    const p = makeProvenance()
    p.events[0].observed_at = PRIVATE_CANARY
    expect(decodeIntentProvenance(p, SUBMISSION_A)).toBeNull()
  })
})
