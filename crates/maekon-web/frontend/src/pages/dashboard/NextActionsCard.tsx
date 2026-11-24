/**
 * "Next up" card for the Dashboard overview (#13735).
 *
 * Lists open to-dos and suggested tasks from the durable task store. Every row
 * opens the Tasks screen, where the item can be confirmed or moved along; this
 * card itself never changes a task.
 */

import { ChevronRight, Lightbulb, ListChecks } from 'lucide-react'
import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { Button, Card, CardTitle, Skeleton } from '../../components/ui'
import { useDurableTasks } from '../../hooks/useDurableTasks'
import { colors, iconSize, interaction, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { IS_TAURI } from '../../utils/platform'
import { buildNextActions, NEXT_ACTIONS_PATH, type NextActionItem } from './nextActions'

const VISIBLE_ITEMS = 3

export type NextActionsStatus = 'loading' | 'ready' | 'error' | 'unavailable'

export interface NextActionsCardProps {
  status: NextActionsStatus
  items: readonly NextActionItem[]
}

// Same state words as the Tasks screen, so a row reads the same in both places.
function ItemLabel({ item }: { item: NextActionItem }) {
  const { t } = useTranslation()
  if (item.kind === 'candidate') return <>{t('dashboard.nextActions.suggested')}</>
  switch (item.state) {
    case 'IN_PROGRESS':
      return <>{t('durableTasks.state.inProgress')}</>
    case 'WAITING':
      return <>{t('durableTasks.state.waiting')}</>
    case 'CONFIRMED':
      return <>{t('durableTasks.state.confirmed')}</>
  }
}

export function NextActionsCard({ status, items }: NextActionsCardProps) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const openTasks = () => navigate(NEXT_ACTIONS_PATH)
  const visible = items.slice(0, VISIBLE_ITEMS)
  const hiddenCount = items.length - visible.length

  return (
    <Card variant="default" padding="md" data-testid="next-actions" aria-busy={status === 'loading' || undefined}>
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <CardTitle>{t('dashboard.nextActions.title')}</CardTitle>
        {status === 'ready' && items.length > 0 && (
          <Button variant="ghost" size="sm" onClick={openTasks}>
            {t('dashboard.nextActions.openAll')}
          </Button>
        )}
      </div>

      {status === 'loading' && <Skeleton className="h-12 w-full" />}

      {status === 'error' && (
        <p role="alert" className={cn(typography.body, 'text-semantic-error')}>
          {t('dashboard.nextActions.loadError')}
        </p>
      )}

      {status === 'unavailable' && (
        <p data-testid="next-actions-unavailable" className={cn(typography.body, colors.text.secondary)}>
          {t('dashboard.nextActions.desktopOnly')}
        </p>
      )}

      {status === 'ready' && items.length === 0 && (
        <div data-testid="next-actions-empty" className="flex flex-col items-start gap-2">
          <p className={cn(typography.label, colors.text.primary)}>{t('dashboard.nextActions.emptyTitle')}</p>
          <p className={cn(typography.body, colors.text.secondary)}>{t('dashboard.nextActions.emptyDescription')}</p>
          <Button variant="secondary" size="sm" onClick={openTasks}>
            {t('dashboard.nextActions.openTasks')}
          </Button>
        </div>
      )}

      {status === 'ready' && items.length > 0 && (
        <>
          <ul className="flex flex-col gap-2">
            {visible.map((item) => {
              const Icon = item.kind === 'candidate' ? Lightbulb : ListChecks
              const title = item.title?.trim() || t('durableTasks.noTitle')
              return (
                <li key={`${item.kind}-${item.id}`}>
                  <button
                    type="button"
                    data-testid={`next-action-${item.kind}-${item.id}`}
                    onClick={openTasks}
                    title={title}
                    className={cn(
                      'flex w-full items-center gap-3 rounded-lg bg-surface-muted px-3 py-2 text-left hover:bg-hover',
                      interaction.interactive,
                      interaction.focusRing,
                    )}
                  >
                    <Icon className={cn(iconSize.base, 'shrink-0 text-brand-text')} aria-hidden="true" />
                    <span className={cn(typography.body, colors.text.primary, 'line-clamp-2 min-w-0 flex-1')}>
                      {title}
                    </span>
                    <span className={cn(typography.caption, colors.text.secondary, 'shrink-0 whitespace-nowrap')}>
                      <ItemLabel item={item} />
                    </span>
                    <ChevronRight className={cn(iconSize.base, 'shrink-0', colors.text.tertiary)} aria-hidden="true" />
                  </button>
                </li>
              )
            })}
          </ul>
          {hiddenCount > 0 && (
            <p className={cn('mt-2', typography.caption, colors.text.tertiary)}>
              {t('dashboard.nextActions.more', { count: hiddenCount })}
            </p>
          )}
        </>
      )}
    </Card>
  )
}

export interface OverviewNextActionsProps {
  /**
   * Render nothing unless there is at least one item. The empty first screen
   * already explains how next steps appear; it only adds this card when the
   * user has something to pick up (e.g. yesterday's tasks, first thing in the
   * morning).
   */
  hideWhenEmpty?: boolean
}

function DesktopNextActions({ hideWhenEmpty }: Required<OverviewNextActionsProps>) {
  const { todos, candidates, loading, error } = useDurableTasks()
  // The task store answers over IPC, so its lists are typed only at compile
  // time. A malformed answer (a null list) is a load error, not an empty list,
  // and must not take the whole first screen down with it.
  const wellFormed = Array.isArray(todos) && Array.isArray(candidates)
  const items = useMemo(() => (wellFormed ? buildNextActions(todos, candidates) : []), [wellFormed, todos, candidates])
  const status: NextActionsStatus = loading ? 'loading' : error || !wellFormed ? 'error' : 'ready'
  if (hideWhenEmpty && (status !== 'ready' || items.length === 0)) return null
  return <NextActionsCard status={status} items={items} />
}

/**
 * The durable task store is reached over Tauri IPC only; the browser build
 * says so instead of reporting an IPC failure as an empty list.
 */
export default function OverviewNextActions({ hideWhenEmpty = false }: OverviewNextActionsProps) {
  if (!IS_TAURI) return hideWhenEmpty ? null : <NextActionsCard status="unavailable" items={[]} />
  return <DesktopNextActions hideWhenEmpty={hideWhenEmpty} />
}
