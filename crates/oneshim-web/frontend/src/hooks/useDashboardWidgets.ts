import { useCallback, useEffect, useState } from 'react'
import { DEFAULT_VISIBILITY, getWidgetsForSection, WIDGET_REGISTRY } from '../components/widget-registry'

const STORAGE_KEY = 'oneshim-dashboard-widget-config'

function loadWidgetConfig(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return { ...DEFAULT_VISIBILITY }
    const parsed = JSON.parse(raw)
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      return { ...DEFAULT_VISIBILITY }
    }
    // Merge: defaults for new widgets, drop removed widget IDs
    const merged: Record<string, boolean> = {}
    for (const [id, defaultVal] of Object.entries(DEFAULT_VISIBILITY)) {
      merged[id] = typeof parsed[id] === 'boolean' ? parsed[id] : defaultVal
    }
    return merged
  } catch {
    return { ...DEFAULT_VISIBILITY }
  }
}

export function useDashboardWidgets() {
  const [config, setConfig] = useState(loadWidgetConfig)

  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(config))
    } catch {
      /* ignore */
    }
  }, [config])

  const isVisible = useCallback((widgetId: string): boolean => config[widgetId] !== false, [config])

  const canToggle = useCallback(
    (widgetId: string): boolean => {
      // Can always toggle ON
      if (!config[widgetId]) return true
      // Check if it's the last visible in its section
      const widget = WIDGET_REGISTRY.find((w) => w.id === widgetId)
      if (!widget) return false
      const sectionWidgets = getWidgetsForSection(widget.section)
      const visibleCount = sectionWidgets.filter((w) => config[w.id] !== false).length
      return visibleCount > 1
    },
    [config],
  )

  const toggle = useCallback((widgetId: string) => {
    setConfig((prev) => {
      // Check canToggle inline to avoid stale closure
      if (prev[widgetId] !== false) {
        const widget = WIDGET_REGISTRY.find((w) => w.id === widgetId)
        if (widget) {
          const sectionWidgets = getWidgetsForSection(widget.section)
          const visibleCount = sectionWidgets.filter((w) => prev[w.id] !== false).length
          if (visibleCount <= 1) return prev
        }
      }
      return { ...prev, [widgetId]: !prev[widgetId] }
    })
  }, [])

  const resetToDefaults = useCallback(() => {
    setConfig({ ...DEFAULT_VISIBILITY })
  }, [])

  return { isVisible, canToggle, toggle, resetToDefaults }
}
