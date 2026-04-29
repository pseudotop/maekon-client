import { PanelLeftClose, Search } from 'lucide-react'
import { useCallback, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from 'react-router-dom'
import {
  getRoutesForGroup,
  joinChildPath,
  navGroups,
  type RouteLeaf,
  useCurrentGroup,
  useCurrentRoute,
} from '../../routes'
import { iconSize, interaction, layout, motion } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { MOD_KEY } from '../../utils/platform'
import TreeView, { type TreeNode } from './TreeView'

interface SidePanelProps {
  collapsed: boolean
  width: number
  onResizeStart: (e: React.MouseEvent) => void
  onResizeByKeyboard?: (delta: number) => void
  /**
   * Collapse handler — rendered as a chevron button in the panel header so
   * users have a discoverable affordance to hide the sub-nav without needing
   * the Cmd/Ctrl+B shortcut or clicking the active ActivityBar icon.
   */
  onCollapse?: () => void
  /** Opens the command palette. Renders a search trigger between header and tree. */
  onSearchOpen?: () => void
}

interface PanelContents {
  headerLabelKey: string
  sidebarNodes: TreeNode[]
  selectedId: string | null
  /** Stable React key — forces TreeView remount when the group switches. */
  treeKey: string
}

export default function SidePanel({
  collapsed,
  width,
  onResizeStart,
  onResizeByKeyboard,
  onCollapse,
  onSearchOpen,
}: SidePanelProps) {
  const navigate = useNavigate()
  const { t } = useTranslation()
  const location = useLocation()
  const activeGroup = useCurrentGroup()
  const { node: currentRoute } = useCurrentRoute()

  // Build the tree to display in the panel.  Two modes:
  //   1. Group mode — activeGroup !== null.  Show every route in the group
  //      as a top-level tree item, with its children nested one level deep.
  //      Node IDs are fully-qualified paths so onSelect can navigate directly.
  //   2. Bottom mode — activeGroup === null (Settings / Privacy).  Fall back
  //      to the legacy "children of current route" view with child paths as
  //      IDs; handleNodeSelect composes the full path from the parent.
  const contents = useMemo<PanelContents | null>(() => {
    if (activeGroup) {
      const group = navGroups.find((g) => g.id === activeGroup)
      if (!group) return null

      const routes = getRoutesForGroup(activeGroup)
      const pathname = location.pathname
      let selectedId: string | null = null

      const nodes: TreeNode[] = routes.map((route) => {
        const node: TreeNode = {
          id: route.path,
          label: t(route.labelKey),
          icon: route.icon ? <route.icon className={cn(iconSize.sm, 'text-content-muted')} /> : undefined,
        }

        // Narrow on `route.children` directly so TypeScript knows it's defined
        // inside the block — avoids the `children!` non-null assertion.
        const routeChildren = route.children
        if (routeChildren && routeChildren.length > 0) {
          node.children = routeChildren.map((child) => {
            const fullPath = joinChildPath(route, child)
            if (!selectedId && (pathname === fullPath || pathname.startsWith(`${fullPath}/`))) {
              selectedId = fullPath
            }
            return { id: fullPath, label: t(child.labelKey) }
          })
        }

        // Leaf route (or fallback when no child matched yet) — promote the
        // parent as the selected node when the current pathname resolves to
        // that parent but not to any of its children.
        if (!selectedId && (pathname === route.path || pathname.startsWith(`${route.path}/`))) {
          selectedId = route.path
        }

        return node
      })

      return {
        headerLabelKey: group.labelKey,
        sidebarNodes: nodes,
        selectedId,
        treeKey: `group:${activeGroup}`,
      }
    }

    // Bottom-item mode: show the active top-level route's children.
    if (!currentRoute.children?.length) return null

    const pathname = location.pathname

    // Grouped bottom-mode: build nested tree from childGroups metadata.
    if (currentRoute.childGroups) {
      const childMap = new Map(currentRoute.children.map((c) => [c.path, c]))
      const nodes: TreeNode[] = currentRoute.childGroups.map((group, gi) => ({
        id: `__group-${gi}`,
        label: t(group.labelKey),
        children: group.tabs
          .map((tab) => childMap.get(tab))
          .filter((c): c is RouteLeaf => Boolean(c))
          .map((child) => ({
            id: joinChildPath(currentRoute, child),
            label: t(child.labelKey),
          })),
      }))
      const selectedId =
        nodes.flatMap((g) => g.children ?? []).find((c) => pathname === c.id || pathname.startsWith(`${c.id}/`))?.id ??
        null
      return {
        headerLabelKey: currentRoute.labelKey,
        sidebarNodes: nodes,
        selectedId,
        treeKey: `route:${currentRoute.path}`,
      }
    }

    // Flat bottom-mode (unchanged fallback for Privacy etc.)
    const nodes: TreeNode[] = currentRoute.children.map((child) => ({
      id: joinChildPath(currentRoute, child),
      label: t(child.labelKey),
    }))

    const selectedId =
      currentRoute.children
        .map((c) => joinChildPath(currentRoute, c))
        .find((p) => pathname === p || pathname.startsWith(`${p}/`)) ?? null

    return {
      headerLabelKey: currentRoute.labelKey,
      sidebarNodes: nodes,
      selectedId,
      treeKey: `route:${currentRoute.path}`,
    }
  }, [activeGroup, currentRoute, location.pathname, t])

  const handleNodeSelect = useCallback(
    (nodeId: string) => {
      if (nodeId.startsWith('__')) return
      navigate(nodeId)
    },
    [navigate],
  )

  const handleResizeKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const STEP = 20
      if (e.key === 'ArrowLeft') {
        e.preventDefault()
        onResizeByKeyboard?.(-STEP)
      } else if (e.key === 'ArrowRight') {
        e.preventDefault()
        onResizeByKeyboard?.(STEP)
      }
    },
    [onResizeByKeyboard],
  )

  // Collapsed OR no contents to show (childless bottom routes like /privacy
  // with no active group) → hide the sidebar entirely rather than leave a
  // phantom 260px column stealing horizontal space from <main>.
  if (collapsed || !contents || contents.sidebarNodes.length === 0) return null

  const collapseLabel = t('shell.collapseSidebar', 'Collapse side panel')

  return (
    <div className="relative flex" style={{ width }}>
      <div className={cn('flex flex-1 flex-col overflow-hidden', layout.sidePanel.bg, layout.sidePanel.border)}>
        <div
          className={cn('flex flex-shrink-0 items-center justify-between gap-2 px-4 py-2', layout.sidePanel.headerBg)}
        >
          {/*
           * aria-live on the header text announces the group label whenever
           * it changes — clicking Monitor → Data swaps the tree contents but
           * a screen-reader user wouldn't otherwise perceive the region
           * change.  "polite" avoids interrupting current announcements.
           */}
          <span className={cn(layout.sidePanel.headerText, 'truncate')} aria-live="polite" aria-atomic="true">
            {t(contents.headerLabelKey)}
          </span>
          {onCollapse && (
            <button
              type="button"
              onClick={onCollapse}
              className={cn(
                'flex flex-shrink-0 items-center justify-center rounded p-0.5',
                'text-content-tertiary hover:bg-hover hover:text-content-strong',
                motion.colors,
                interaction.focusRing,
              )}
              aria-label={collapseLabel}
              title={collapseLabel}
              data-testid="sidepanel-collapse"
            >
              <PanelLeftClose className={iconSize.sm} aria-hidden="true" />
            </button>
          )}
        </div>

        {onSearchOpen && (
          <div className="px-2 py-1">
            <button
              type="button"
              onClick={onSearchOpen}
              className={cn(
                'flex w-full items-center gap-2 px-2 py-1.5',
                layout.sidePanel.searchTrigger,
                motion.colors,
                interaction.focusRing,
              )}
              aria-label={t('shell.searchShortcut', { key: MOD_KEY, defaultValue: `Search (${MOD_KEY}+K)` })}
              data-testid="sidepanel-search"
            >
              <Search className={cn(iconSize.sm, 'flex-shrink-0')} aria-hidden="true" />
              <span className="flex-1 truncate text-left">{t('shell.search', 'Search...')}</span>
              <span className={layout.commandPalette.badge} aria-hidden="true">
                {MOD_KEY}K
              </span>
            </button>
          </div>
        )}

        <div className="flex-1 overflow-y-auto px-1 py-1">
          <TreeView
            key={contents.treeKey}
            nodes={contents.sidebarNodes}
            selectedId={contents.selectedId ?? undefined}
            onSelect={handleNodeSelect}
          />
        </div>
      </div>

      {/* biome-ignore lint/a11y/useSemanticElements: separator role on div is intentional for resizable panel — no native <hr> equivalent for interactive vertical separator */}
      <div
        className={cn('flex-shrink-0', layout.sidePanel.resizeHandle, interaction.focusRing)}
        onMouseDown={onResizeStart}
        onKeyDown={handleResizeKeyDown}
        role="separator"
        aria-orientation="vertical"
        aria-valuenow={width}
        aria-valuemin={layout.sidePanel.minWidth}
        aria-valuemax={layout.sidePanel.maxWidth}
        tabIndex={0}
        aria-label={t('shell.resizeSidebar', 'Resize sidebar')}
      />
    </div>
  )
}

SidePanel.displayName = 'SidePanel'
