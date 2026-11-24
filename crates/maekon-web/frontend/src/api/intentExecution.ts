import type {
  IntentEventKind,
  IntentExecutionEvent,
  IntentExecutionProvenance,
  IntentProviderAttempt,
} from './contracts'

const providers = ['local_model', 'remote_model', 'rule', 'unknown'] as const
const revisions = ['unknown', 'not_applicable'] as const
const planning = ['llm', 'rule', 'unsupported', 'denied', 'error', 'unknown'] as const
const actions = ['not_started', 'denied', 'completed', 'failed', 'unknown'] as const
const reasons = [
  'denied',
  'timeout',
  'unavailable',
  'invalid_output',
  'provider_error',
  'confidence_rejected',
  'unsupported_action',
  'action_failed',
  'observation_unavailable',
  'legacy_unknown',
] as const

function member<T extends readonly string[]>(value: unknown, values: T): value is T[number] {
  return typeof value === 'string' && values.includes(value)
}

function object(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function fields(value: Record<string, unknown>, keys: string[]): boolean {
  const actual = Object.keys(value)
  return actual.length === keys.length && keys.every((key) => actual.includes(key))
}

export function isOpaqueIntentId(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value) &&
    value !== '00000000-0000-0000-0000-000000000000'
  )
}

const nullableId = (value: unknown) => value === null || isOpaqueIntentId(value)
const safeReason = (value: unknown) => value === null || member(value, reasons)

function decodeAttempt(value: unknown): IntentProviderAttempt | null {
  const a = object(value)
  if (
    !a ||
    !fields(a, [
      'attempt_id',
      'predecessor',
      'effective_provider_class',
      'model_revision',
      'outcome',
      'reason',
      'capabilities',
    ])
  )
    return null
  const c = object(a.capabilities)
  if (!c || !fields(c, ['skills_requested', 'skills_context_forwarded', 'skill_context_kind', 'skills_effective']))
    return null
  if (
    !isOpaqueIntentId(a.attempt_id) ||
    !nullableId(a.predecessor) ||
    !member(a.effective_provider_class, providers) ||
    !member(a.model_revision, revisions) ||
    !member(a.outcome, ['succeeded', 'denied', 'timeout', 'invalid_output', 'unavailable', 'error', 'unknown']) ||
    !safeReason(a.reason) ||
    typeof c.skills_requested !== 'boolean' ||
    (c.skills_context_forwarded !== null && typeof c.skills_context_forwarded !== 'boolean') ||
    !member(c.skill_context_kind, ['none', 'catalog', 'activated']) ||
    !member(c.skills_effective, ['used', 'not_used', 'unsupported', 'unknown'])
  )
    return null
  // Every key and value is bounded above; copy rather than retain the raw input.
  return { ...a, capabilities: { ...c } } as unknown as IntentProviderAttempt
}

function decodeKind(value: unknown, attempts: Set<string>): IntentEventKind | null {
  const k = object(value)
  if (!k) return null
  switch (k.stage) {
    case 'started':
    case 'action_started':
      return fields(k, ['stage']) ? { stage: k.stage } : null
    case 'attempt_started':
    case 'attempt_finished':
      return fields(k, ['stage', 'attempt_id']) && isOpaqueIntentId(k.attempt_id) && attempts.has(k.attempt_id)
        ? { stage: k.stage, attempt_id: k.attempt_id }
        : null
    case 'planning_finished':
      return fields(k, ['stage', 'outcome']) && member(k.outcome, planning)
        ? { stage: k.stage, outcome: k.outcome }
        : null
    case 'action_finished':
      return fields(k, ['stage', 'outcome']) && member(k.outcome, actions)
        ? { stage: k.stage, outcome: k.outcome }
        : null
    default:
      return null
  }
}

function decodeEvents(value: unknown, executionId: string, attempts: Set<string>): IntentExecutionEvent[] | null {
  if (!Array.isArray(value) || value.length > 32) return null
  const bySequence = new Map<number, IntentExecutionEvent>()
  const byId = new Map<string, IntentExecutionEvent>()
  for (const raw of value) {
    const e = object(raw)
    if (
      !e ||
      !fields(e, ['execution_id', 'event_id', 'sequence', 'observed_at', 'kind']) ||
      e.execution_id !== executionId ||
      !isOpaqueIntentId(e.event_id) ||
      typeof e.sequence !== 'number' ||
      !Number.isInteger(e.sequence) ||
      e.sequence < 0 ||
      e.sequence >= 32 ||
      typeof e.observed_at !== 'string' ||
      e.observed_at.length > 35 ||
      !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})$/.test(e.observed_at) ||
      !Number.isFinite(Date.parse(e.observed_at))
    )
      return null
    const kind = decodeKind(e.kind, attempts)
    if (!kind) return null
    const event: IntentExecutionEvent = {
      execution_id: executionId,
      event_id: e.event_id,
      sequence: e.sequence,
      observed_at: e.observed_at,
      kind,
    }
    for (const prior of [bySequence.get(event.sequence), byId.get(event.event_id)]) {
      if (prior && JSON.stringify(prior) !== JSON.stringify(event)) return null
    }
    bySequence.set(event.sequence, event)
    byId.set(event.event_id, event)
  }
  return [...bySequence.values()].sort((a, b) => a.sequence - b.sequence)
}

/** Decode only the response-bound v1 observation; configuration/health are never inputs. */
export function decodeIntentProvenance(value: unknown, submissionId?: string): IntentExecutionProvenance | null {
  const p = object(value)
  if (
    !p ||
    !fields(p, [
      'schema_version',
      'execution_id',
      'submission_id',
      'retry_of',
      'config_revision',
      'requested_provider_class',
      'configuration_reason',
      'attempts',
      'planning_outcome',
      'action_outcome',
      'observation',
      'observation_reason',
      'storage',
      'events',
    ])
  )
    return null
  if (
    p.schema_version !== 'maekon.intent_execution.v1' ||
    p.storage !== 'process_memory' ||
    !isOpaqueIntentId(p.execution_id) ||
    !nullableId(p.submission_id) ||
    !nullableId(p.retry_of) ||
    (submissionId !== undefined && (!isOpaqueIntentId(submissionId) || p.submission_id !== submissionId)) ||
    !member(p.config_revision, revisions) ||
    !member(p.requested_provider_class, providers) ||
    (p.configuration_reason !== null && p.configuration_reason !== 'rule_only_degradation') ||
    !member(p.planning_outcome, planning) ||
    !member(p.action_outcome, actions) ||
    !member(p.observation, ['complete', 'partial', 'legacy_unknown']) ||
    !safeReason(p.observation_reason) ||
    !Array.isArray(p.attempts) ||
    p.attempts.length > 8
  )
    return null
  const attempts: IntentProviderAttempt[] = []
  const ids = new Set<string>()
  for (const raw of p.attempts) {
    const attempt = decodeAttempt(raw)
    if (!attempt || ids.has(attempt.attempt_id) || (attempt.predecessor !== null && !ids.has(attempt.predecessor)))
      return null
    ids.add(attempt.attempt_id)
    attempts.push(attempt)
  }
  const events = decodeEvents(p.events, p.execution_id, ids)
  if (!events) return null
  const last = events[events.length - 1]
  const complete =
    events.every((event, index) => event.sequence === index) &&
    events[0]?.kind.stage === 'started' &&
    last?.kind.stage === 'action_finished' &&
    last.kind.outcome === p.action_outcome &&
    events.filter((event) => event.kind.stage === 'action_finished').length === 1
  const incomplete = p.observation === 'complete' && !complete
  return {
    ...p,
    attempts,
    events,
    observation: incomplete ? 'partial' : p.observation,
    observation_reason: incomplete ? 'observation_unavailable' : p.observation_reason,
  } as unknown as IntentExecutionProvenance
}

export function intentProvenanceFromError(error: unknown, submissionId: string): IntentExecutionProvenance | null {
  return decodeIntentProvenance(object(error)?.executionProvenance, submissionId)
}

/** Missing, duplicate and malformed mappings all remain unknown. */
export function decodeExactIntentLookup(value: unknown, submissionId: string): IntentExecutionProvenance | null {
  if (!Array.isArray(value) || value.length !== 1) return null
  return decodeIntentProvenance(object(value[0])?.execution_provenance, submissionId)
}
