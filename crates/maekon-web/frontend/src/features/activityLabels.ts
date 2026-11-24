/**
 * Display labels for activity types (#13738).
 *
 * The day, week and recalibration screens used to hard-code English names for
 * the default regime choices and to turn wire work-type ids such as
 * `ACTIVE_CODING` into English words. Both now read `regimeOptions.*` and
 * `workTypes.*`, so every locale shows its own label.
 */

type Translate = (key: string) => string

export interface RegimeOption {
  id: string
  label: string
}

/** Regime choices offered before the server has classified any segment. The ids are wire values. */
const DEFAULT_REGIMES: ReadonlyArray<{ readonly id: string; readonly key: string }> = [
  { id: 'deep-work', key: 'regimeOptions.deepWork' },
  { id: 'communication', key: 'regimeOptions.communication' },
  { id: 'meeting', key: 'regimeOptions.meeting' },
  { id: 'break', key: 'regimeOptions.break' },
  { id: 'admin', key: 'regimeOptions.admin' },
]

export function defaultRegimeOptions(t: Translate): RegimeOption[] {
  return DEFAULT_REGIMES.map(({ id, key }) => ({ id, label: t(key) }))
}

/** Label of a default regime id, or `undefined` when the id is not one of the defaults. */
export function defaultRegimeLabel(id: string, t: Translate): string | undefined {
  const regime = DEFAULT_REGIMES.find((candidate) => candidate.id === id)
  return regime ? t(regime.key) : undefined
}

/** Every wire `WorkType` value (`maekon-core` `models/tiered_memory/content.rs`, SCREAMING_SNAKE_CASE). */
export const KNOWN_WORK_TYPES = [
  'ACTIVE_CODING',
  'CODE_REVIEW',
  'WRITING',
  'READING',
  'DESIGNING',
  'FORM_FILLING',
  'BROWSING',
  'PASSIVE_MEETING',
  'ACTIVE_MEETING',
  'NAVIGATION',
  'TERMINAL_COMMANDS',
  'LOG_READING',
  'DOCUMENT_WRITING',
  'DOCUMENT_READING',
  'CHAT_COMPOSING',
  'UNKNOWN',
] as const

export type KnownWorkType = (typeof KNOWN_WORK_TYPES)[number]

function isKnownWorkType(value: string): value is KnownWorkType {
  return (KNOWN_WORK_TYPES as readonly string[]).includes(value)
}

/** "NEW_WORK_TYPE" -> "New Work Type", for a value newer than this build. */
function humanizeWireId(value: string): string {
  return value
    .toLowerCase()
    .split('_')
    .filter(Boolean)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ')
}

/**
 * Display label for a wire work type. A value this build does not know yet
 * reads as plain words instead of a raw id until a `workTypes.*` key is added.
 */
export function workTypeLabel(workType: string, t: Translate): string {
  return isKnownWorkType(workType) ? t(`workTypes.${workType}`) : humanizeWireId(workType)
}
