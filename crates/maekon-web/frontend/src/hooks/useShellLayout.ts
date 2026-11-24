import { useCallback, useEffect, useRef, useState } from 'react'
import { layout } from '../styles/tokens'

const STORAGE_KEY_WIDTH = 'maekon-sidebar-width'
const STORAGE_KEY_COLLAPSED = 'maekon-sidebar-collapsed'

/**
 * #13736: at or below this window width the side panel starts collapsed so the
 * main content keeps its room (the window's minimum width is 800px). The user
 * can still open it; that choice lasts until the window crosses the line again.
 */
export const NARROW_SHELL_QUERY = '(max-width: 900px)'

function matchesNarrowShell(): boolean {
  try {
    return window.matchMedia?.(NARROW_SHELL_QUERY).matches ?? false
  } catch {
    return false
  }
}

function loadPersistedWidth(): number {
  try {
    const raw = localStorage.getItem(STORAGE_KEY_WIDTH)
    const parsed = Number(raw)
    return raw && Number.isFinite(parsed)
      ? Math.min(Math.max(parsed, layout.sidePanel.minWidth), layout.sidePanel.maxWidth)
      : layout.sidePanel.defaultWidth
  } catch {
    return layout.sidePanel.defaultWidth
  }
}

function loadPersistedCollapsed(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY_COLLAPSED) === 'true'
  } catch {
    return false
  }
}

export function useShellLayout() {
  // Lazy initializers — called only once on mount
  const [sidebarWidth, setSidebarWidth] = useState(loadPersistedWidth)
  // The user's own wide-window choice. Only this one is persisted, so a
  // narrow window never overwrites it.
  const [preferCollapsed, setPreferCollapsed] = useState(loadPersistedCollapsed)
  const [isNarrow, setIsNarrow] = useState(matchesNarrowShell)
  // Whether the user opened the panel while the window is narrow.
  const [narrowOpen, setNarrowOpen] = useState(false)
  const [isResizing, setIsResizing] = useState(false)
  const startXRef = useRef(0)
  const startWidthRef = useRef(0)

  const sidebarCollapsed = isNarrow ? !narrowOpen : preferCollapsed

  useEffect(() => {
    const query = window.matchMedia?.(NARROW_SHELL_QUERY)
    if (!query) return
    const onChange = (event: MediaQueryListEvent) => {
      setIsNarrow(event.matches)
      // Entering the narrow range collapses again, even if the panel had
      // been opened the last time the window was this narrow.
      if (event.matches) setNarrowOpen(false)
    }
    setIsNarrow(query.matches)
    query.addEventListener?.('change', onChange)
    return () => query.removeEventListener?.('change', onChange)
  }, [])

  // Persist to localStorage — skip during active mouse resize to avoid hundreds of writes
  useEffect(() => {
    if (isResizing) return
    try {
      localStorage.setItem(STORAGE_KEY_WIDTH, String(sidebarWidth))
      localStorage.setItem(STORAGE_KEY_COLLAPSED, String(preferCollapsed))
    } catch {
      /* ignore */
    }
  }, [sidebarWidth, preferCollapsed, isResizing])

  // NOTE: --sidebar-width is set by AppShell, not here.
  // AppShell combines sidebarCollapsed with the active route's sub-nav
  // presence so routes like /day with no children collapse the CSS grid
  // column to 0 instead of leaving a phantom 260px cell behind.

  const toggleSidebar = useCallback(() => {
    if (isNarrow) setNarrowOpen((prev) => !prev)
    else setPreferCollapsed((prev) => !prev)
  }, [isNarrow])

  // Stable ref — does not depend on sidebarWidth (reads startWidthRef at drag time)
  const onResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault()
    startXRef.current = e.clientX
    // Capture current width from the DOM CSS variable (source of truth during drag)
    const raw = getComputedStyle(document.documentElement).getPropertyValue('--sidebar-width')
    startWidthRef.current = parseInt(raw, 10) || layout.sidePanel.defaultWidth
    setIsResizing(true)
  }, [])

  useEffect(() => {
    if (!isResizing) return

    const onMouseMove = (e: MouseEvent) => {
      const delta = e.clientX - startXRef.current
      const newWidth = Math.min(
        Math.max(startWidthRef.current + delta, layout.sidePanel.minWidth),
        layout.sidePanel.maxWidth,
      )
      setSidebarWidth(newWidth)
    }

    const onMouseUp = () => {
      setIsResizing(false)
    }

    document.addEventListener('mousemove', onMouseMove)
    document.addEventListener('mouseup', onMouseUp)
    document.body.style.cursor = 'col-resize'
    document.body.style.userSelect = 'none'

    return () => {
      document.removeEventListener('mousemove', onMouseMove)
      document.removeEventListener('mouseup', onMouseUp)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
    }
  }, [isResizing])

  const onResizeByKeyboard = useCallback((delta: number) => {
    setSidebarWidth((prev) => {
      const next = Math.min(Math.max(prev + delta, layout.sidePanel.minWidth), layout.sidePanel.maxWidth)
      return next
    })
  }, [])

  return {
    sidebarWidth,
    sidebarCollapsed,
    toggleSidebar,
    onResizeStart,
    onResizeByKeyboard,
  }
}
