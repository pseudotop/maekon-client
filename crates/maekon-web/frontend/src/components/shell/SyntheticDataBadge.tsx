/**
 * Persistent "synthetic demo data" label (#9611 WD-02.3).
 *
 * Rendered by the app shell, above the router, so no route change, modal, or
 * route-level error boundary can unmount it. That placement is the requirement:
 * a viewer who sees a screenshot of any screen in this session must be able to
 * tell the data is synthetic, not only on the one page that fetched it.
 *
 * The claim is evidence-backed and latched — see `syntheticSessionSignal`.
 *
 * ## Not colour alone
 *
 * The badge carries an icon and a word, not just an amber background. A viewer
 * with a colour-vision difference, a greyscale screenshot, or a high-contrast
 * theme still reads "demo data". `role="status"` puts it in the accessibility
 * tree as a live announcement when it first appears, and `aria-label` spells
 * out the abbreviated visible text for screen readers.
 */

import { FlaskConical } from 'lucide-react'
import { useSyncExternalStore } from 'react'
import { useTranslation } from 'react-i18next'
import { iconSize, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { readSyntheticSession, subscribeSyntheticSession } from './syntheticSessionSignal'

/**
 * Stable server-snapshot getter for `useSyncExternalStore`.
 *
 * SSR/prerender never has a latched session, and returning a fresh object here
 * would make React see a changed snapshot on every render.
 */
const readServerSnapshot = () => null

export function SyntheticDataBadge() {
  const { t } = useTranslation()
  const evidence = useSyncExternalStore(subscribeSyntheticSession, readSyntheticSession, readServerSnapshot)

  // No evidence yet: say nothing. Asserting "synthetic" about a session that
  // might be connected to a real server would be the opposite error, and a
  // pre-evidence shell is not one of the transitions this badge must survive.
  if (!evidence) return null

  const label = t('contextHome.syntheticBadge.label', 'Demo data')
  const description = t(
    'contextHome.syntheticBadge.description',
    'This session is showing synthetic demonstration data, not real records.',
  )

  return (
    <div
      data-testid="synthetic-data-badge"
      role="status"
      aria-label={description}
      title={description}
      className={cn(
        'pointer-events-none fixed top-1.5 left-1/2 z-tooltip -translate-x-1/2',
        'flex items-center gap-1.5 rounded-full px-2.5 py-0.5',
        'border border-semantic-warning/60 bg-semantic-warning/20 text-semantic-warning',
        typography.weight.medium,
        'tracking-wide',
      )}
    >
      <FlaskConical className={cn(iconSize.xs, 'shrink-0')} aria-hidden="true" />
      <span>{label}</span>
    </div>
  )
}
