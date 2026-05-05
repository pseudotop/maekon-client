import { useEffect, useState } from 'react'

interface CaptureFlashProps {
  timestamp: string | null
}

/** Brief full-screen border flash on manual capture success. */
export function CaptureFlash({ timestamp }: CaptureFlashProps) {
  const [visible, setVisible] = useState(false)

  useEffect(() => {
    if (!timestamp) return
    setVisible(true)
    const timer = setTimeout(() => setVisible(false), 400)
    return () => clearTimeout(timer)
  }, [timestamp])

  if (!visible) return null

  return (
    <div
      className="pointer-events-none fixed inset-0 z-tooltip border-4 border-brand"
      style={{ opacity: visible ? 1 : 0, transition: 'opacity 200ms ease-out' }}
    />
  )
}
