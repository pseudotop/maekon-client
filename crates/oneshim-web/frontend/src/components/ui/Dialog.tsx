/**
 *
 */
import { createContext, type ReactNode, useContext, useEffect, useId, useRef } from 'react'
import { elevation, layout, motion, radius, typography } from '../../styles/tokens'
import { dialogVariants } from '../../styles/variants'
import { cn } from '../../utils/cn'

const DialogContext = createContext<string | undefined>(undefined)

export interface DialogProps {
  open: boolean
  onClose: () => void
  children: ReactNode
}

export function Dialog({ open, onClose, children }: DialogProps) {
  const previousFocusRef = useRef<HTMLElement | null>(null)

  useEffect(() => {
    if (open) {
      previousFocusRef.current = document.activeElement as HTMLElement
      document.body.style.overflow = 'hidden'
    } else {
      document.body.style.overflow = ''
      previousFocusRef.current?.focus()
    }
    return () => {
      document.body.style.overflow = ''
    }
  }, [open])

  useEffect(() => {
    if (!open) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      }
    }
    document.addEventListener('keydown', handler)
    return () => document.removeEventListener('keydown', handler)
  }, [open, onClose])

  if (!open) return null

  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: Escape handled via document keydown
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-outside close
    <div
      className={cn('fixed inset-0 z-overlay flex items-center justify-center', layout.commandPalette.overlay)}
      onClick={onClose}
    >
      {children}
    </div>
  )
}

export interface DialogContentProps extends React.HTMLAttributes<HTMLDivElement> {
  size?: keyof typeof dialogVariants.size
}

export function DialogContent({ className, size = 'md', children, ...props }: DialogContentProps) {
  const ref = useRef<HTMLDivElement>(null)
  const titleId = useId()

  // Focus trap
  useEffect(() => {
    const el = ref.current
    if (!el) return

    const handleTab = (e: KeyboardEvent) => {
      if (e.key !== 'Tab') return
      const focusable = el.querySelectorAll<HTMLElement>(
        'input, button, textarea, select, a[href], [tabindex]:not([tabindex="-1"])',
      )
      if (focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault()
        last.focus()
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault()
        first.focus()
      }
    }

    // Auto-focus first focusable element
    const timer = setTimeout(() => {
      const firstFocusable = el.querySelector<HTMLElement>(
        'input, button, textarea, select, a[href], [tabindex]:not([tabindex="-1"])',
      )
      firstFocusable?.focus()
    }, 50)

    document.addEventListener('keydown', handleTab)
    return () => {
      clearTimeout(timer)
      document.removeEventListener('keydown', handleTab)
    }
  }, [])

  return (
    <DialogContext.Provider value={titleId}>
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: click stops propagation only — keyboard handled by Dialog */}
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className={cn(
          'w-full',
          radius.lg,
          elevation.dialog,
          motion.opacity,
          layout.commandPalette.bg,
          layout.commandPalette.border,
          dialogVariants.size[size],
          className,
        )}
        onClick={(e) => e.stopPropagation()}
        {...props}
      >
        {children}
      </div>
    </DialogContext.Provider>
  )
}

export function DialogTitle({ className, id, ...props }: React.HTMLAttributes<HTMLHeadingElement>) {
  const contextId = useContext(DialogContext)
  return <h2 id={id ?? contextId} className={cn('p-4 pb-0 text-content', typography.h3, className)} {...props} />
}

export function DialogBody({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('p-4 text-content-secondary text-sm', className)} {...props} />
}

export function DialogFooter({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('flex justify-end gap-2 border-DEFAULT border-t p-4', className)} {...props} />
}
