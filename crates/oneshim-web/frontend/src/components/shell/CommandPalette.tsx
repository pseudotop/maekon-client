import type { TFunction } from 'i18next'
import { Moon, PanelLeft, Search, Sun } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { useTheme } from '../../contexts/ThemeContext'
import { type RouteNode, routeTree } from '../../routes'
import { iconSize, layout, motion } from '../../styles/tokens'
import { cn } from '../../utils/cn'

interface PaletteItem {
  id: string
  /** Pre-resolved label used for filtering. Recomputed each render via useMemo. */
  label: string
  icon: React.ReactNode
  type: 'page' | 'action'
  action: () => void
}

interface CommandPaletteProps {
  isOpen: boolean
  onClose: () => void
  onToggleSidebar: () => void
}

const LISTBOX_ID = 'command-palette-listbox'

/**
 * Build a flat list of navigable palette entries from routeTree.
 *
 * For each top-level route we emit one "parent" entry, and for every child
 * one deep-link entry whose label is `Parent > Child`. This keeps the palette
 * in lockstep with `routeTree` — if we add a route, the palette picks it up
 * automatically. No more hardcoded drift like the pre-refactor version that
 * was missing 7 top-level routes.
 */
function buildNavigationItems(
  nodes: readonly RouteNode[],
  navigate: (path: string) => void,
  t: TFunction,
): PaletteItem[] {
  const items: PaletteItem[] = []

  for (const node of nodes) {
    if (!node.icon) continue
    const Icon = node.icon
    const parentLabel = t(node.labelKey)
    const icon = <Icon className={iconSize.base} aria-hidden="true" />

    // Parent entry — also acts as the defaultChild redirect.
    items.push({
      id: `route-${node.path}`,
      label: parentLabel,
      icon,
      type: 'page',
      action: () => navigate(node.path),
    })

    // Deep-link entries for every child so users can jump to a sub-tab
    // without going through the parent.
    if (node.children) {
      const basePath = node.path === '/' ? '' : node.path
      for (const child of node.children) {
        items.push({
          id: `route-${node.path}-${child.path}`,
          label: `${parentLabel} › ${t(child.labelKey)}`,
          icon,
          type: 'page',
          action: () => navigate(`${basePath}/${child.path}`),
        })
      }
    }
  }

  return items
}

export default function CommandPalette({ isOpen, onClose, onToggleSidebar }: CommandPaletteProps) {
  const navigate = useNavigate()
  const navigateRef = useRef(navigate)
  navigateRef.current = navigate
  const { t } = useTranslation()
  const { theme, toggleTheme } = useTheme()
  const [query, setQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const dialogRef = useRef<HTMLDivElement>(null)

  const items = useMemo<PaletteItem[]>(() => {
    const nav = (path: string) => navigateRef.current(path)
    const pages = buildNavigationItems(routeTree, nav, t)
    const actions: PaletteItem[] = [
      {
        id: 'theme',
        label:
          theme === 'dark'
            ? t('shell.switchToLight', 'Switch to Light Mode')
            : t('shell.switchToDark', 'Switch to Dark Mode'),
        icon:
          theme === 'dark' ? (
            <Sun className={iconSize.base} aria-hidden="true" />
          ) : (
            <Moon className={iconSize.base} aria-hidden="true" />
          ),
        type: 'action',
        action: toggleTheme,
      },
      {
        id: 'sidebar',
        label: t('shell.toggleSidebar', 'Toggle Sidebar'),
        icon: <PanelLeft className={iconSize.base} aria-hidden="true" />,
        type: 'action',
        action: onToggleSidebar,
      },
    ]
    return [...pages, ...actions]
  }, [theme, toggleTheme, onToggleSidebar, t])

  const filtered = useMemo(() => {
    if (!query) return items
    const q = query.toLowerCase()
    return items.filter((item) => item.label.toLowerCase().includes(q))
  }, [items, query])

  const activeDescendant = filtered[selectedIndex] ? `palette-option-${filtered[selectedIndex].id}` : undefined

  useEffect(() => {
    if (isOpen) {
      setQuery('')
      setSelectedIndex(0)
      const timer = setTimeout(() => inputRef.current?.focus(), 50)
      return () => clearTimeout(timer)
    }
  }, [isOpen])

  useEffect(() => {
    if (selectedIndex >= filtered.length) {
      setSelectedIndex(Math.max(0, filtered.length - 1))
    }
  }, [filtered.length, selectedIndex])

  // Focus trap — keep keyboard focus inside the dialog.
  //
  // Unlike the previous revision, we also handle the "focus escaped the
  // dialog" case: if Tab fires while the currently-focused element is
  // outside the dialog (e.g. the autofocus timer has not yet run, or the
  // user clicked something behind the backdrop), we intercept and pull
  // focus back to the first focusable inside. That was the root cause of
  // the `shell-command-palette.spec.ts P018` skip — with only a single
  // focusable (the combobox input), the original trap silently let Tab
  // walk the page behind the dialog whenever activeElement was neither
  // first nor last.
  useEffect(() => {
    if (!isOpen) return

    const handleFocusTrap = (e: KeyboardEvent) => {
      if (e.key !== 'Tab' || !dialogRef.current) return

      const focusable = dialogRef.current.querySelectorAll<HTMLElement>(
        'input, button, [tabindex]:not([tabindex="-1"])',
      )
      if (focusable.length === 0) return

      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      const active = document.activeElement
      const focusInDialog = active instanceof Node && dialogRef.current.contains(active)

      if (!focusInDialog) {
        e.preventDefault()
        first.focus()
        return
      }

      if (e.shiftKey && active === first) {
        e.preventDefault()
        last.focus()
      } else if (!e.shiftKey && active === last) {
        e.preventDefault()
        first.focus()
      }
    }

    document.addEventListener('keydown', handleFocusTrap)
    return () => document.removeEventListener('keydown', handleFocusTrap)
  }, [isOpen])

  const executeItem = (item: PaletteItem) => {
    item.action()
    onClose()
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault()
        setSelectedIndex((prev) => (prev + 1) % filtered.length)
        break
      case 'ArrowUp':
        e.preventDefault()
        setSelectedIndex((prev) => (prev - 1 + filtered.length) % filtered.length)
        break
      case 'Enter':
        e.preventDefault()
        if (filtered[selectedIndex]) executeItem(filtered[selectedIndex])
        break
      case 'Escape':
        e.preventDefault()
        onClose()
        break
    }
  }

  if (!isOpen) return null

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop overlay — Escape handled in input onKeyDown
    // biome-ignore lint/a11y/useKeyWithClickEvents: Escape key handled via input onKeyDown handler
    <div
      className={cn(
        'fixed inset-0 z-overlay flex items-start justify-center',
        layout.commandPalette.position,
        layout.commandPalette.overlay,
      )}
      onClick={onClose}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: onClick only prevents bubble to backdrop, not interactive */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        data-testid="command-palette"
        aria-label={t('commandPalette.dialogLabel', 'Command Palette')}
        className={cn(
          layout.commandPalette.width,
          layout.commandPalette.bg,
          layout.commandPalette.border,
          layout.commandPalette.shadow,
          'overflow-hidden rounded-lg',
        )}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center border-muted border-b px-4 py-3">
          <Search className={cn('mr-3 flex-shrink-0 text-content-muted', iconSize.base)} aria-hidden="true" />
          <input
            ref={inputRef}
            type="text"
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            data-testid="command-palette-input"
            role="combobox"
            aria-expanded={true}
            aria-haspopup="listbox"
            aria-controls={LISTBOX_ID}
            aria-activedescendant={activeDescendant}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value)
              setSelectedIndex(0)
            }}
            onKeyDown={handleKeyDown}
            placeholder={t('commandPalette.placeholder', 'Type a command or search...')}
            aria-label={t('commandPalette.placeholder', 'Type a command or search...')}
            className={cn('flex-1 outline-none', layout.commandPalette.input)}
          />
        </div>

        <div className="max-h-80 overflow-y-auto py-1" role="listbox" id={LISTBOX_ID}>
          {filtered.length === 0 ? (
            <div className="px-4 py-6 text-center text-content-muted text-sm">
              {t('commandPalette.noResults', 'No results found')}
            </div>
          ) : (
            filtered.map((item, index) => (
              // biome-ignore lint/a11y/useKeyWithClickEvents: combobox APG — keyboard handled at input level (ArrowDown/Up/Enter)
              <div
                key={item.id}
                id={`palette-option-${item.id}`}
                role="option"
                aria-selected={index === selectedIndex}
                tabIndex={-1}
                onClick={() => executeItem(item)}
                onMouseEnter={() => setSelectedIndex(index)}
                className={cn(
                  'flex w-full cursor-pointer items-center gap-3 px-4 py-2 text-left',
                  motion.colors,
                  layout.commandPalette.itemText,
                  index === selectedIndex && layout.commandPalette.itemActive,
                  layout.commandPalette.itemBg,
                )}
              >
                <span className="flex-shrink-0 text-content-muted">{item.icon}</span>
                <span className="flex-1 truncate">{item.label}</span>
                <span className={layout.commandPalette.badge}>{item.type}</span>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  )
}

CommandPalette.displayName = 'CommandPalette'
