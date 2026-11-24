import { ChevronDown, ChevronRight } from 'lucide-react'
import { useCallback, useRef, useState } from 'react'
import { iconSize, interaction, layout, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

export interface TreeNode {
  id: string
  label: string
  icon?: React.ReactNode
  count?: number
  children?: TreeNode[]
}

/**
 * Which parents start expanded.
 * - `all`: every parent, at every depth (the Settings and Privacy trees).
 * - `selected`: only the ancestors of `selectedId`. When `selectedId` later
 *   moves under a collapsed parent, those ancestors expand so the selection
 *   stays visible. Used by the main tree, whose bundles start folded (#13736).
 */
export type TreeExpandMode = 'all' | 'selected'

interface TreeViewProps {
  nodes: TreeNode[]
  selectedId?: string
  onSelect?: (id: string) => void
  expandMode?: TreeExpandMode
}

function collectParentIds(nodes: TreeNode[], into: Set<string>): Set<string> {
  for (const node of nodes) {
    if (node.children?.length) {
      into.add(node.id)
      collectParentIds(node.children, into)
    }
  }
  return into
}

/** Ids of the parents on the way to `id`, outermost first; `null` when `id` is not in the tree. */
function findAncestorIds(nodes: TreeNode[], id: string): string[] | null {
  for (const node of nodes) {
    if (node.id === id) return []
    if (node.children?.length) {
      const below = findAncestorIds(node.children, id)
      if (below) return [node.id, ...below]
    }
  }
  return null
}

function initialExpanded(nodes: TreeNode[], mode: TreeExpandMode, selectedId: string | undefined): Set<string> {
  if (mode === 'all') return collectParentIds(nodes, new Set())
  return new Set(selectedId ? (findAncestorIds(nodes, selectedId) ?? []) : [])
}

export default function TreeView({ nodes, selectedId, onSelect, expandMode = 'all' }: TreeViewProps) {
  // Expansion lives at the root so every depth shares one state: keyboard
  // expand/collapse works on nested parents too, and a nested parent keeps its
  // state when an outer parent is folded and unfolded again.
  const [expanded, setExpanded] = useState<Set<string>>(() => initialExpanded(nodes, expandMode, selectedId))
  const [revealedFor, setRevealedFor] = useState(selectedId)
  const treeRef = useRef<HTMLDivElement>(null)

  // Reveal a selection that moved under a collapsed parent (a command-palette
  // jump, a deep link, an in-page link). Adjusting state during render keeps
  // the first paint correct instead of flashing the folded tree.
  if (selectedId !== revealedFor) {
    setRevealedFor(selectedId)
    if (expandMode === 'selected' && selectedId) {
      const ancestors = findAncestorIds(nodes, selectedId) ?? []
      if (ancestors.some((id) => !expanded.has(id))) {
        setExpanded(new Set([...expanded, ...ancestors]))
      }
    }
  }

  const toggleExpand = useCallback((id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  // Update roving tabindex: move tabIndex=0 to the focused item
  const updateRovingTabIndex = useCallback((focusedEl: HTMLElement) => {
    if (!treeRef.current) return
    const items = treeRef.current.querySelectorAll<HTMLElement>('[role="treeitem"]')
    items.forEach((item) => {
      item.tabIndex = item === focusedEl ? 0 : -1
    })
  }, [])

  // Arrow-key navigation for the ARIA tree pattern, handled once at the root
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (!treeRef.current) return
      const items = Array.from(treeRef.current.querySelectorAll<HTMLElement>('[role="treeitem"]'))
      const currentIndex = items.indexOf(e.target as HTMLElement)
      if (currentIndex === -1) return

      const focusAndTrack = (el: HTMLElement | undefined) => {
        if (!el) return
        el.focus()
        updateRovingTabIndex(el)
      }

      switch (e.key) {
        case 'ArrowDown': {
          e.preventDefault()
          focusAndTrack(items[currentIndex + 1])
          break
        }
        case 'ArrowUp': {
          e.preventDefault()
          focusAndTrack(items[currentIndex - 1])
          break
        }
        case 'ArrowRight': {
          e.preventDefault()
          const btn = items[currentIndex]
          const nodeId = btn.dataset.nodeId
          if (nodeId && !expanded.has(nodeId)) {
            toggleExpand(nodeId)
          } else {
            focusAndTrack(items[currentIndex + 1])
          }
          break
        }
        case 'ArrowLeft': {
          e.preventDefault()
          const btn = items[currentIndex]
          const nodeId = btn.dataset.nodeId
          if (nodeId && expanded.has(nodeId)) {
            // Open parent node: collapse it
            toggleExpand(nodeId)
          } else {
            // Leaf or closed parent: move focus to parent treeitem
            const currentLevel = parseInt(btn.getAttribute('aria-level') || '1', 10)
            if (currentLevel > 1) {
              // Walk backwards to find the first item at a shallower level (the parent)
              for (let i = currentIndex - 1; i >= 0; i--) {
                const level = parseInt(items[i].getAttribute('aria-level') || '1', 10)
                if (level < currentLevel) {
                  focusAndTrack(items[i])
                  break
                }
              }
            }
            // currentLevel === 1: no-op per APG Tree Pattern — focus stays
          }
          break
        }
        case 'Home': {
          e.preventDefault()
          focusAndTrack(items[0])
          break
        }
        case 'End': {
          e.preventDefault()
          focusAndTrack(items[items.length - 1])
          break
        }
      }
    },
    [expanded, toggleExpand, updateRovingTabIndex],
  )

  return (
    <div ref={treeRef} className="text-sm" role="tree" onKeyDown={handleKeyDown}>
      <TreeLevel
        nodes={nodes}
        depth={0}
        expanded={expanded}
        onToggle={toggleExpand}
        selectedId={selectedId}
        onSelect={onSelect}
      />
    </div>
  )
}

TreeView.displayName = 'TreeView'

interface TreeLevelProps {
  nodes: TreeNode[]
  depth: number
  expanded: ReadonlySet<string>
  onToggle: (id: string) => void
  selectedId?: string
  onSelect?: (id: string) => void
}

function TreeLevel({ nodes, depth, expanded, onToggle, selectedId, onSelect }: TreeLevelProps) {
  return (
    <>
      {nodes.map((node, index) => {
        const hasChildren = node.children && node.children.length > 0
        const isExpanded = expanded.has(node.id)
        const isSelected = selectedId === node.id

        return (
          <div key={node.id} role="none" className={depth === 0 && index > 0 ? 'mt-1' : undefined}>
            <button
              type="button"
              role="treeitem"
              aria-expanded={hasChildren ? isExpanded : undefined}
              aria-selected={isSelected}
              aria-level={depth + 1}
              data-node-id={hasChildren ? node.id : undefined}
              tabIndex={depth === 0 && index === 0 ? 0 : -1}
              onClick={() => {
                if (hasChildren) onToggle(node.id)
                onSelect?.(node.id)
              }}
              className={cn(
                'flex w-full items-center gap-1.5 rounded-sm px-2 py-1',
                motion.colors,
                isSelected ? layout.sidePanel.itemActive : layout.sidePanel.itemBg,
                isSelected
                  ? 'text-sm'
                  : depth === 0 && hasChildren
                    ? layout.sidePanel.itemTextParent
                    : depth >= 1
                      ? layout.sidePanel.itemTextChild
                      : layout.sidePanel.itemText,
                interaction.focusRing,
              )}
              style={{ paddingLeft: `${depth * 16 + 8}px` }}
            >
              {hasChildren ? (
                isExpanded ? (
                  <ChevronDown className={cn(iconSize.sm, 'flex-shrink-0 text-content-muted')} aria-hidden="true" />
                ) : (
                  <ChevronRight className={cn(iconSize.sm, 'flex-shrink-0 text-content-muted')} aria-hidden="true" />
                )
              ) : (
                <span className={cn(iconSize.sm, 'flex-shrink-0')} />
              )}
              {node.icon && <span className="flex-shrink-0">{node.icon}</span>}
              <span className="flex-1 truncate text-left">{node.label}</span>
              {node.count !== undefined && (
                <span className={cn(typography.nano, 'text-content-muted tabular-nums')}>{node.count}</span>
              )}
            </button>
            {hasChildren && isExpanded && (
              <div role="none" className={layout.sidePanel.childBorder}>
                {/* biome-ignore lint/a11y/useSemanticElements: the ARIA tree pattern nests treeitems in role="group"; <fieldset> groups form controls */}
                <div className="text-sm" role="group">
                  <TreeLevel
                    nodes={node.children ?? []}
                    depth={depth + 1}
                    expanded={expanded}
                    onToggle={onToggle}
                    selectedId={selectedId}
                    onSelect={onSelect}
                  />
                </div>
              </div>
            )}
          </div>
        )
      })}
    </>
  )
}
