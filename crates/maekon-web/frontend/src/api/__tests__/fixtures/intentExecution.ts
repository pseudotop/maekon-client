// Synthetic wire observations only; these fixtures do not execute a model or native action.
import type {
  ExecuteIntentHintResponse,
  IntentEventKind,
  IntentExecutionProvenance,
  IntentProviderAttempt,
} from '../../contracts'

export const opaqueId = (value: number) => `00000000-0000-4000-8000-${value.toString(16).padStart(12, '0')}`
export const SUBMISSION_A = opaqueId(1)
export const SUBMISSION_B = opaqueId(2)
export const EXECUTION_A = opaqueId(11)
export const EXECUTION_B = opaqueId(12)
export const PRIVATE_CANARY = 'PRIVATE_CANARY https://secret.invalid /private/token WINDOW_TITLE PROMPT_TEXT'

export function makeAttempt(overrides: Partial<IntentProviderAttempt> = {}): IntentProviderAttempt {
  return {
    attempt_id: opaqueId(21),
    predecessor: null,
    effective_provider_class: 'remote_model',
    model_revision: 'unknown',
    outcome: 'succeeded',
    reason: null,
    capabilities: {
      skills_requested: true,
      skills_context_forwarded: true,
      skill_context_kind: 'catalog',
      skills_effective: 'unknown',
    },
    ...overrides,
  }
}

export function makeProvenance(overrides: Partial<IntentExecutionProvenance> = {}): IntentExecutionProvenance {
  const executionId = overrides.execution_id ?? EXECUTION_A
  const attempts = overrides.attempts ?? [makeAttempt()]
  const planning = overrides.planning_outcome ?? 'llm'
  const action = overrides.action_outcome ?? 'completed'
  const kinds: IntentEventKind[] = [{ stage: 'started' }]
  for (const a of attempts)
    kinds.push(
      { stage: 'attempt_started', attempt_id: a.attempt_id },
      { stage: 'attempt_finished', attempt_id: a.attempt_id },
    )
  kinds.push({ stage: 'planning_finished', outcome: planning })
  if (action !== 'not_started') kinds.push({ stage: 'action_started' })
  kinds.push({ stage: 'action_finished', outcome: action })
  return {
    schema_version: 'maekon.intent_execution.v1',
    execution_id: executionId,
    submission_id: SUBMISSION_A,
    retry_of: null,
    config_revision: 'unknown',
    requested_provider_class: 'remote_model',
    configuration_reason: null,
    attempts,
    planning_outcome: planning,
    action_outcome: action,
    observation: 'complete',
    observation_reason: null,
    storage: 'process_memory',
    events: kinds.map((kind, sequence) => ({
      execution_id: executionId,
      event_id: opaqueId(100 + sequence),
      sequence,
      observed_at: '2026-10-05T12:00:00Z',
      kind,
    })),
    ...overrides,
  }
}

export function makeFallback(): IntentExecutionProvenance {
  const first = makeAttempt({ outcome: 'unavailable', reason: 'unavailable' })
  return makeProvenance({
    planning_outcome: 'rule',
    attempts: [
      first,
      makeAttempt({
        attempt_id: opaqueId(22),
        predecessor: first.attempt_id,
        effective_provider_class: 'rule',
        model_revision: 'not_applicable',
        capabilities: {
          skills_requested: true,
          skills_context_forwarded: false,
          skill_context_kind: 'none',
          skills_effective: 'unsupported',
        },
      }),
    ],
  })
}

export function makeResponse(
  provenance: IntentExecutionProvenance | null = makeProvenance(),
): ExecuteIntentHintResponse {
  return {
    command_id: 'same-command',
    session_id: 'same-session',
    execution_provenance: provenance,
    planned_intent: { Hotkey: { keys: ['Ctrl', 'S'] } },
    result: { success: true, element: null, verification: null, retry_count: 0, elapsed_ms: 42, error: null },
  }
}
