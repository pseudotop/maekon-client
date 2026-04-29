import {
  Activity,
  AppWindow,
  BarChart3,
  Cpu,
  Focus,
  Grid3x3,
  LineChart,
  List,
  type LucideIcon,
  RefreshCw,
  Sun,
} from 'lucide-react'

export type SectionId = 'overview' | 'monitoring' | 'insights'

export interface WidgetDef {
  id: string
  section: SectionId
  labelKey: string
  icon: LucideIcon
}

export const WIDGET_REGISTRY: WidgetDef[] = [
  { id: 'overview.realtime', section: 'overview', labelKey: 'widgets.realtime', icon: Activity },
  { id: 'overview.today-summary', section: 'overview', labelKey: 'widgets.todaySummary', icon: Sun },
  { id: 'overview.stat-cards', section: 'overview', labelKey: 'widgets.statCards', icon: BarChart3 },
  { id: 'monitoring.metrics-chart', section: 'monitoring', labelKey: 'widgets.metricsChart', icon: LineChart },
  { id: 'monitoring.app-usage', section: 'monitoring', labelKey: 'widgets.appUsage', icon: AppWindow },
  { id: 'monitoring.process-list', section: 'monitoring', labelKey: 'widgets.processList', icon: List },
  { id: 'insights.focus-widget', section: 'insights', labelKey: 'widgets.focusWidget', icon: Focus },
  { id: 'insights.update-panel', section: 'insights', labelKey: 'widgets.updatePanel', icon: RefreshCw },
  { id: 'insights.heatmap', section: 'insights', labelKey: 'widgets.heatmap', icon: Grid3x3 },
  { id: 'insights.system-status', section: 'insights', labelKey: 'widgets.systemStatus', icon: Cpu },
]

export const DEFAULT_VISIBILITY: Record<string, boolean> = Object.fromEntries(WIDGET_REGISTRY.map((w) => [w.id, true]))

export function getWidgetsForSection(section: SectionId): WidgetDef[] {
  return WIDGET_REGISTRY.filter((w) => w.section === section)
}
