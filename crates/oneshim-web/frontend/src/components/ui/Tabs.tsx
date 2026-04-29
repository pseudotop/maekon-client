import { type KeyboardEvent, type ReactNode, useRef } from 'react'
import { interaction, motion, radius, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

export interface Tab {
  id: string
  label: string
  icon?: ReactNode
  disabled?: boolean
}

export interface TabsProps {
  tabs: Tab[]
  activeTab: string
  onTabChange: (id: string) => void
  className?: string
  ariaLabel?: string
  idBase?: string
  /** Insert extra right margin after the tab at this index to visually separate groups. */
  groupBreakAfter?: number
}

function getEnabledIndexes(tabs: Tab[]) {
  return tabs.reduce<number[]>((indexes, tab, index) => {
    if (!tab.disabled) {
      indexes.push(index)
    }

    return indexes
  }, [])
}

export function Tabs({ tabs, activeTab, onTabChange, className, ariaLabel, idBase, groupBreakAfter }: TabsProps) {
  const buttonRefs = useRef<Array<HTMLButtonElement | null>>([])
  const enabledIndexes = getEnabledIndexes(tabs)
  const activeIndex = tabs.findIndex((tab) => tab.id === activeTab && !tab.disabled)
  const focusableIndex = activeIndex >= 0 ? activeIndex : (enabledIndexes[0] ?? -1)

  const focusAndSelectTab = (index: number) => {
    const tab = tabs[index]
    if (!tab || tab.disabled) {
      return
    }

    onTabChange(tab.id)
    buttonRefs.current[index]?.focus()
  }

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (enabledIndexes.length === 0) {
      return
    }

    const enabledIndexPosition = enabledIndexes.indexOf(index)
    if (enabledIndexPosition === -1) {
      return
    }

    let nextIndex: number | null = null

    switch (event.key) {
      case 'ArrowRight':
      case 'ArrowDown':
        nextIndex = enabledIndexes[(enabledIndexPosition + 1) % enabledIndexes.length]
        break
      case 'ArrowLeft':
      case 'ArrowUp':
        nextIndex = enabledIndexes[(enabledIndexPosition - 1 + enabledIndexes.length) % enabledIndexes.length]
        break
      case 'Home':
        nextIndex = enabledIndexes[0]
        break
      case 'End':
        nextIndex = enabledIndexes[enabledIndexes.length - 1]
        break
      default:
        return
    }

    event.preventDefault()
    focusAndSelectTab(nextIndex)
  }

  return (
    <div
      role="tablist"
      aria-label={ariaLabel}
      className={cn(
        'inline-flex flex-wrap items-center gap-1 border border-muted bg-surface-muted/80 p-1',
        radius.lg,
        className,
      )}
    >
      {tabs.map((tab, index) => {
        const isActive = index === activeIndex || (activeIndex === -1 && index === focusableIndex)
        const tabId = idBase ? `${idBase}-tab-${tab.id}` : undefined
        const panelId = idBase ? `${idBase}-panel-${tab.id}` : undefined

        return (
          <button
            key={tab.id}
            id={tabId}
            ref={(node) => {
              buttonRefs.current[index] = node
            }}
            type="button"
            role="tab"
            aria-selected={isActive}
            aria-controls={panelId}
            tabIndex={isActive ? 0 : -1}
            disabled={tab.disabled}
            onClick={() => {
              focusAndSelectTab(index)
            }}
            onKeyDown={(event) => {
              handleKeyDown(event, index)
            }}
            className={cn(
              `inline-flex items-center gap-2 px-3 py-2 ${typography.weight.medium}`,
              radius.md,
              typography.body,
              interaction.interactive,
              interaction.focusRing,
              motion.colors,
              isActive
                ? 'bg-surface-elevated text-content shadow-[0_1px_2px_rgb(var(--content)/0.1)]'
                : 'text-content-secondary hover:bg-surface-elevated/70 hover:text-content',
              tab.disabled &&
                'cursor-not-allowed text-content-tertiary opacity-60 hover:bg-transparent hover:text-content-tertiary',
              index === groupBreakAfter && 'mr-4',
            )}
          >
            {tab.icon}
            <span>{tab.label}</span>
          </button>
        )
      })}
    </div>
  )
}
