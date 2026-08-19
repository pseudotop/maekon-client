/**
 * Section-level derivations for the context home (#9611 WD-02.3).
 *
 * Pure functions over the #9625 snapshot. They exist so the page renders a
 * decision rather than making one inline, and so the decisions are testable
 * without a DOM.
 *
 * ## "Empty" and "unavailable" are the server's words, not ours
 *
 * Each section carries its own `status`. This module never infers a section's
 * state from `items.length` — a section that could not be served returns an
 * empty list *and* `status: 'unavailable'`, and treating the empty list as the
 * signal is exactly the conflation the server contract was shaped to prevent.
 * Length is consulted only to tell `ready`-with-nothing from `ready`-with-rows.
 */

import type { ContextHomeProjectSection, ContextHomeSnapshot, ContextHomeThreadSection } from '../../api/contextHome'

export type AnySection = ContextHomeThreadSection | ContextHomeProjectSection

/**
 * What a single section should render as.
 *
 * `unknown` is the open-union escape hatch: the server may introduce a status
 * this build has never heard of, and blanking the section — or worse, drawing
 * it as empty — would be a claim we cannot support. It renders as its own
 * treatment that says so.
 */
export type SectionRenderState = 'ready' | 'empty' | 'unavailable' | 'unknown'

/**
 * Whole-page completeness, used for the one-line summary and the live-region
 * announcement.
 *
 * `partial` is the state this slice most needs to keep visible: some sections
 * answered and at least one could not. A page that renders that as ordinary
 * success is quietly under-reporting the user's actual context.
 */
export type HomeCompleteness = 'complete' | 'partial' | 'empty' | 'unavailable'

/** Rows rendered per section before the list is truncated in the UI. */
export const MAX_RENDERED_THREADS = 8
/** Rows rendered in the projects section before truncation. */
export const MAX_RENDERED_PROJECTS = 6
/**
 * Participant chips rendered per thread before collapsing into a "+N" count.
 *
 * The server already caps participants at 12 per thread, so this is not a
 * safety bound — it is a legibility one. A 50-person org routinely produces
 * threads whose participant strip is longer than the subject it belongs to.
 */
export const MAX_RENDERED_PARTICIPANTS = 4

export function sectionRenderState(section: AnySection | undefined): SectionRenderState {
  if (!section) return 'unknown'
  switch (section.status) {
    case 'ready':
      return section.items.length > 0 ? 'ready' : 'empty'
    case 'empty':
      return 'empty'
    case 'unavailable':
      return 'unavailable'
    default:
      return 'unknown'
  }
}

/**
 * True when the server could not serve this section — as distinct from having
 * nothing to put in it.
 *
 * `unknown` counts as not-served: an unrecognised status is not a licence to
 * present the section as answered.
 */
export function isUnserved(state: SectionRenderState): boolean {
  return state === 'unavailable' || state === 'unknown'
}

export function homeCompleteness(snapshot: ContextHomeSnapshot): HomeCompleteness {
  const states = [
    sectionRenderState(snapshot.mail),
    sectionRenderState(snapshot.messenger),
    sectionRenderState(snapshot.projects),
  ]

  if (states.every(isUnserved)) return 'unavailable'
  if (states.some(isUnserved)) return 'partial'
  if (states.every((s) => s === 'empty')) return 'empty'
  return 'complete'
}

/** How many rows a section is holding back from the rendered list. */
export function hiddenRowCount(section: AnySection | undefined, cap: number): number {
  if (!section) return 0
  return Math.max(0, section.items.length - cap)
}

/**
 * Format `as_of` in the zone the **server** intends, not the viewer's.
 *
 * The demo's day boundaries are KST; rendering a snapshot timestamp in browser
 * local time silently moves items across days for anyone outside that zone,
 * which turns a correct snapshot into a wrong-looking one. The zone comes from
 * the snapshot rather than a constant so a differently-configured tenant is
 * rendered in its own terms.
 *
 * Falls back to the raw ISO string if the runtime rejects the zone — an
 * unformatted-but-true timestamp beats a formatted-but-wrong one, and beats
 * throwing inside a render.
 */
export function formatAsOf(isoUtc: string, timeZone: string, locale: string): string {
  try {
    return new Intl.DateTimeFormat(locale, {
      timeZone,
      dateStyle: 'medium',
      timeStyle: 'short',
    }).format(new Date(isoUtc))
  } catch {
    return isoUtc
  }
}
