import { useEffect, useRef } from 'react'
import { useNavigate } from 'react-router-dom'
import { MOD_KEY } from '../utils/platform'

interface ShortcutHandlers {
  onHelp?: () => void
  onEscape?: () => void
  onToggleSidebar?: () => void
  onTogglePalette?: () => void
  onArrowLeft?: () => void
  onArrowRight?: () => void
  onArrowUp?: () => void
  onArrowDown?: () => void
  onEnter?: () => void
  onSpace?: () => void
}

export function useKeyboardShortcuts(handlers: ShortcutHandlers = {}, enabled = true) {
  const navigate = useNavigate()
  const navigateRef = useRef(navigate)
  navigateRef.current = navigate
  const handlersRef = useRef(handlers)
  handlersRef.current = handlers

  useEffect(() => {
    if (!enabled) return

    const handleKeyDown = (event: KeyboardEvent) => {
      // Guard: skip during IME composition (Korean, Japanese, Chinese input)
      if (event.isComposing) return

      const h = handlersRef.current

      // Cmd/Ctrl shortcuts — work even when focused in inputs
      if (event.metaKey || event.ctrlKey) {
        if (event.key === 'w') {
          event.preventDefault()
          // Close-to-tray: hide window instead of closing the app
          import('@tauri-apps/api/window')
            .then((m) => m.getCurrentWindow().hide())
            .catch((e) => console.debug('window.hide failed (browser fallback):', e))
          return
        }
        if (event.key === 'b') {
          event.preventDefault()
          h.onToggleSidebar?.()
          return
        }
        if (event.key === 'k') {
          event.preventDefault()
          h.onTogglePalette?.()
          return
        }
      }

      const target = event.target as HTMLElement
      if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable) {
        if (event.key === 'Escape' && h.onEscape) {
          h.onEscape()
        }
        return
      }

      switch (event.key.toLowerCase()) {
        case 'd':
          event.preventDefault()
          navigateRef.current('/overview')
          break
        case 't':
          event.preventDefault()
          navigateRef.current('/timeline/all')
          break
        case 's':
          event.preventDefault()
          navigateRef.current('/settings/general')
          break
        case 'p':
          event.preventDefault()
          navigateRef.current('/privacy/data')
          break
        case '?':
          event.preventDefault()
          h.onHelp?.()
          break
        case '/':
          if (event.shiftKey) {
            event.preventDefault()
            h.onHelp?.()
          }
          break
      }

      switch (event.key) {
        case 'Escape':
          h.onEscape?.()
          break
        case 'ArrowLeft':
          if (h.onArrowLeft) {
            event.preventDefault()
            h.onArrowLeft()
          }
          break
        case 'ArrowRight':
          if (h.onArrowRight) {
            event.preventDefault()
            h.onArrowRight()
          }
          break
        case 'ArrowUp':
          if (h.onArrowUp) {
            event.preventDefault()
            h.onArrowUp()
          }
          break
        case 'ArrowDown':
          if (h.onArrowDown) {
            event.preventDefault()
            h.onArrowDown()
          }
          break
        case 'Enter':
          h.onEnter?.()
          break
        case ' ':
          if (h.onSpace) {
            event.preventDefault()
            h.onSpace()
          }
          break
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [enabled])
}

export function getShortcutsList() {
  return [
    { key: 'D', descriptionKey: 'shortcuts.dashboard' },
    { key: 'T', descriptionKey: 'shortcuts.timeline' },
    { key: 'S', descriptionKey: 'shortcuts.settings' },
    { key: 'P', descriptionKey: 'shortcuts.privacy' },
    { key: '?', descriptionKey: 'shortcuts.help' },
    { key: 'ESC', descriptionKey: 'shortcuts.escape' },
    { key: '\u2190 \u2192', descriptionKey: 'shortcuts.arrows' },
    { key: 'Enter', descriptionKey: 'shortcuts.enter' },
    { key: `${MOD_KEY}W`, descriptionKey: 'shortcuts.closeToTray' },
    { key: `${MOD_KEY}B`, descriptionKey: 'shortcuts.toggleSidebar' },
    { key: `${MOD_KEY}K`, descriptionKey: 'shortcuts.commandPalette' },
  ]
}
