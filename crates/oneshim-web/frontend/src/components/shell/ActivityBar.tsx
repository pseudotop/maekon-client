import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from 'react-router-dom'
import { matchesRoute, type NavGroup, navGroups, type RouteNode, routeTree, useCurrentGroup } from '../../routes'
import { interaction, layout, motion } from '../../styles/tokens'
import { cn } from '../../utils/cn'

const ACTIVITYBAR_WIDTH_PX = 48
const TOOLTIP_ID = 'activity-bar-tooltip'

// Bottom items stay as direct top-level icons (Settings, Privacy).  They are
// heavily used entry points and keeping them one click away avoids the cost
// of the category→sidepanel indirection the main groups pay.
const bottomItems = routeTree.filter((r) => r.bottom && r.icon)

// Stable testid suffix: strip leading "/" and replace "/" with "-".
function pathToId(path: string): string {
  if (path === '/') return 'dashboard'
  return path.slice(1).replace(/\//g, '-')
}

interface ActivityBarProps {
  onToggleSidebar: () => void
  sidebarCollapsed: boolean
}

export default function ActivityBar({ onToggleSidebar, sidebarCollapsed }: ActivityBarProps) {
  const location = useLocation()
  const navigate = useNavigate()
  const { t } = useTranslation()
  const activeGroup = useCurrentGroup()
  const [tooltip, setTooltip] = useState<string | null>(null)
  const [tooltipY, setTooltipY] = useState(0)

  const setTooltipFromEvent = useCallback((label: string, el: HTMLElement) => {
    setTooltip(label)
    setTooltipY(el.getBoundingClientRect().top)
  }, [])
  const clearTooltip = useCallback(() => setTooltip(null), [])

  const handleGroupClick = useCallback(
    (group: NavGroup) => {
      if (activeGroup === group.id) {
        // Already inside this group — toggle the SidePanel (VS Code style).
        onToggleSidebar()
        return
      }
      navigate(group.defaultPath)
      if (sidebarCollapsed) onToggleSidebar()
    },
    [activeGroup, navigate, onToggleSidebar, sidebarCollapsed],
  )

  const handleBottomClick = useCallback(
    (node: RouteNode) => {
      if (matchesRoute(node, location.pathname)) {
        onToggleSidebar()
        return
      }
      navigate(node.path)
      if (sidebarCollapsed) onToggleSidebar()
    },
    [location.pathname, navigate, onToggleSidebar, sidebarCollapsed],
  )

  const renderGroupButton = (group: NavGroup) => {
    const Icon = group.icon
    const active = activeGroup === group.id
    const label = t(group.labelKey)

    return (
      <button
        type="button"
        key={group.id}
        data-testid={`nav-group-${group.id}`}
        onClick={() => handleGroupClick(group)}
        onMouseEnter={(e) => setTooltipFromEvent(label, e.currentTarget)}
        onMouseLeave={clearTooltip}
        onFocus={(e) => setTooltipFromEvent(label, e.currentTarget)}
        onBlur={clearTooltip}
        className={cn(
          'relative flex h-11 w-full items-center justify-center',
          motion.colors,
          active ? layout.activityBar.iconActive : layout.activityBar.iconDefault,
          !active && 'hover:text-content-strong',
          interaction.focusRing,
        )}
        aria-current={active ? 'page' : undefined}
        aria-describedby={tooltip ? TOOLTIP_ID : undefined}
        aria-label={label}
        title={label}
      >
        {active && (
          <div className={cn('absolute top-1.5 bottom-1.5 left-0 w-0.5 rounded-r', layout.activityBar.indicator)} />
        )}
        <Icon className={layout.activityBar.iconSize} aria-hidden="true" />
      </button>
    )
  }

  const renderBottomButton = (node: RouteNode) => {
    if (!node.icon) return null
    const Icon = node.icon
    const active = matchesRoute(node, location.pathname)
    const label = t(node.labelKey)
    const id = pathToId(node.path)

    return (
      <button
        type="button"
        key={node.path}
        data-testid={`nav-${id}`}
        onClick={() => handleBottomClick(node)}
        onMouseEnter={(e) => setTooltipFromEvent(label, e.currentTarget)}
        onMouseLeave={clearTooltip}
        onFocus={(e) => setTooltipFromEvent(label, e.currentTarget)}
        onBlur={clearTooltip}
        className={cn(
          'relative flex h-11 w-full items-center justify-center',
          motion.colors,
          active ? layout.activityBar.iconActive : layout.activityBar.iconDefault,
          !active && 'hover:text-content-strong',
          interaction.focusRing,
        )}
        aria-current={active ? 'page' : undefined}
        aria-describedby={tooltip ? TOOLTIP_ID : undefined}
        aria-label={label}
        title={label}
      >
        {active && (
          <div className={cn('absolute top-1.5 bottom-1.5 left-0 w-0.5 rounded-r', layout.activityBar.indicator)} />
        )}
        <Icon className={layout.activityBar.iconSize} aria-hidden="true" />
      </button>
    )
  }

  return (
    <nav
      className={cn(
        'flex flex-col items-center py-1',
        layout.activityBar.bg,
        layout.activityBar.border,
        layout.activityBar.width,
      )}
      aria-label={t('nav.mainNavLabel', 'Main Navigation')}
    >
      {navGroups.map(renderGroupButton)}

      <div className="flex-1" />

      {bottomItems.map(renderBottomButton)}

      {tooltip && (
        <div
          id={TOOLTIP_ID}
          className={cn('pointer-events-none fixed z-tooltip', layout.activityBar.tooltip)}
          style={{ left: ACTIVITYBAR_WIDTH_PX + 8, top: tooltipY + 4 }}
          role="tooltip"
        >
          {tooltip}
        </div>
      )}
    </nav>
  )
}

ActivityBar.displayName = 'ActivityBar'
