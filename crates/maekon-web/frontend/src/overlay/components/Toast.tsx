import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { SupportDetails } from '../../components/SupportDetails'
import { typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import type { ToastItem } from '../types'

const MAX_VISIBLE = 3
const MAX_PENDING = 20
const TOAST_DURATION = 4000

const typeStyles: Record<ToastItem['type'], string> = {
  success: 'bg-semantic-success/90 text-content-inverse',
  error: 'bg-semantic-error/90 text-content-inverse',
  info: 'bg-surface-sunken/95 text-content-primary border border-border-default',
}

let addToastGlobal: ((toast: Omit<ToastItem, 'id'>) => void) | null = null

export function showToast(
  message: string,
  type: ToastItem['type'] = 'info',
  duration?: number,
  detail?: string | null,
) {
  addToastGlobal?.({ message, type, duration, detail })
}

export function ToastContainer() {
  const { t } = useTranslation()
  const [visible, setVisible] = useState<ToastItem[]>([])
  const [pinned, setPinned] = useState<ReadonlySet<string>>(new Set())
  const pendingRef = useRef<ToastItem[]>([])

  const promoteFromQueue = useCallback(() => {
    setVisible((prev) => {
      if (pendingRef.current.length === 0 || prev.length >= MAX_VISIBLE) return prev
      const next = pendingRef.current.shift()
      if (!next) return prev
      return [...prev, next]
    })
  }, [])

  const addToast = useCallback((toast: Omit<ToastItem, 'id'>) => {
    const id = `${Date.now()}-${Math.random().toString(36).slice(2, 6)}`
    const item: ToastItem = { ...toast, id }

    setVisible((prev) => {
      if (prev.length >= MAX_VISIBLE) {
        // Queue overflow — hold until a slot opens (bounded to MAX_PENDING).
        if (pendingRef.current.length >= MAX_PENDING) pendingRef.current.shift()
        pendingRef.current.push(item)
        return prev
      }
      return [...prev, item]
    })
  }, [])

  useEffect(() => {
    addToastGlobal = addToast
    return () => {
      addToastGlobal = null
    }
  }, [addToast])

  const dismiss = useCallback(
    (id: string) => {
      setVisible((prev) => prev.filter((toast) => toast.id !== id))
      setPinned((prev) => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
      queueMicrotask(promoteFromQueue)
    },
    [promoteFromQueue],
  )

  // Keep the existing oldest-first timer, while an opened diagnostic remains
  // available until explicitly dismissed. Other visible toasts still expire.
  useEffect(() => {
    const oldest = visible.find((toast) => !pinned.has(toast.id))
    if (!oldest) return
    const timer = setTimeout(() => dismiss(oldest.id), oldest.duration ?? TOAST_DURATION)
    return () => clearTimeout(timer)
  }, [visible, pinned, dismiss])

  return (
    <output className="pointer-events-none fixed right-4 bottom-4 z-50 flex flex-col gap-2" aria-live="polite">
      {visible.map((toast) => (
        <div
          key={toast.id}
          className={cn(
            `rounded-lg px-4 py-2 text-xs ${typography.weight.medium} shadow-lg backdrop-blur-sm`,
            'pointer-events-auto animate-[slideIn_200ms_ease-out]',
            typeStyles[toast.type],
          )}
        >
          <p>{toast.message}</p>
          <SupportDetails
            detail={toast.detail}
            onToggle={(open) => {
              if (open) setPinned((prev) => new Set([...prev, toast.id]))
            }}
          />
          <button type="button" aria-label={t('common.close')} onClick={() => dismiss(toast.id)}>
            {t('common.close')}
          </button>
        </div>
      ))}
    </output>
  )
}
