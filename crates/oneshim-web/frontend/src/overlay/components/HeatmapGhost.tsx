import { useEffect, useRef } from 'react'

interface HeatmapPayload {
  grid: number[]
  cols: number
  rows: number
}

/** Warm-to-hot color gradient: transparent → green → yellow → red */
function valueToColor(v: number): string {
  if (v < 0.05) return 'transparent'
  // Green (low) → Yellow (mid) → Red (high)
  const r = v < 0.5 ? Math.floor(v * 2 * 255) : 255
  const g = v < 0.5 ? 255 : Math.floor((1 - (v - 0.5) * 2) * 255)
  const alpha = 0.15 + v * 0.35 // 0.15 → 0.50
  return `rgba(${r}, ${g}, 0, ${alpha})`
}

export default function HeatmapGhost() {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  // Keep canvas resolution in sync with its CSS layout size
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return

    const ro = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect
      canvas.width = width
      canvas.height = height
    })
    ro.observe(canvas)
    return () => ro.disconnect()
  }, [])

  useEffect(() => {
    let unlisten: (() => void) | null = null

    async function setup() {
      const { listen } = await import('@tauri-apps/api/event')

      unlisten = await listen<HeatmapPayload>('overlay:heatmap-update', (e) => {
        const canvas = canvasRef.current
        if (!canvas) return

        const { grid, cols, rows } = e.payload
        const ctx = canvas.getContext('2d')
        if (!ctx) return

        const w = canvas.width
        const h = canvas.height
        const cellW = w / cols
        const cellH = h / rows

        ctx.clearRect(0, 0, w, h)

        for (let row = 0; row < rows; row++) {
          for (let col = 0; col < cols; col++) {
            const v = grid[row * cols + col]
            if (v < 0.05) continue
            ctx.fillStyle = valueToColor(v)
            ctx.fillRect(col * cellW, row * cellH, cellW, cellH)
          }
        }
      })
    }

    setup()
    return () => {
      unlisten?.()
    }
  }, [])

  return <canvas ref={canvasRef} className="pointer-events-none fixed inset-0 h-full w-full" style={{ zIndex: 0 }} />
}
