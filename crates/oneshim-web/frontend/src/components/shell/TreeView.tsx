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

interface TreeViewProps {
  nodes: TreeNode[]
  selectedId?: string
  onSelect?: (id: string) => void
  depth?: number
}

export default function TreeView({ nodes, selectedId, onSelect, depth = 0 }: TreeViewProps) {
  const [expanded, setExpanded] = useState<Set<string>>(() => {
    return new Set(nodes.filter((n) => n.children?.length).map((n) => n.id))
  })
  const treeRef = useRef<HTMLDivElement>(null)

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

  // Arrow-key navigation for ARIA tree pattern (root level only)
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
    // biome-ignore lint/a11y/noStaticElementInteractions: role="tree" is interactive; onKeyDown only at root depth for roving tabindex
    <div
      ref={depth === 0 ? treeRef : undefined}
      className="text-sm"
      role={depth === 0 ? 'tree' : 'group'}
      onKeyDown={depth === 0 ? handleKeyDown : undefined}
    >
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
                if (hasChildren) toggleExpand(node.id)
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
                <TreeView nodes={node.children ?? []} selectedId={selectedId} onSelect={onSelect} depth={depth + 1} />
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}

TreeView.displayName = 'TreeView'
