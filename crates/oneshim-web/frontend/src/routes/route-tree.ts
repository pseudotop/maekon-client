import {
  BarChart3,
  BookOpen,
  Calendar,
  ClipboardList,
  Clock,
  FileText,
  Gauge,
  Image,
  Info,
  LayoutDashboard,
  Lightbulb,
  MessageCircle,
  MessageSquare,
  Monitor,
  RefreshCw,
  Settings,
  Shield,
  Tag,
  Wrench,
  Zap,
} from 'lucide-react'
import type { ComponentType, LazyExoticComponent } from 'react'
import { lazy } from 'react'

export interface RouteNode {
  path: string
  labelKey: string
  icon?: ComponentType<{ className?: string }>
  defaultChild?: string
  component: LazyExoticComponent<ComponentType> | ComponentType
  children?: RouteLeaf[]
  group?: 'monitor' | 'insights' | 'manage'
  bottom?: boolean
  /**
   * When true, RouteRenderer does NOT wrap the component in a RouteErrorBoundary.
   * The component is responsible for its own error boundary placement — useful
   * when stateful providers (e.g., SettingsFormProvider) need to live ABOVE
   * the boundary so their state survives recovery reset.
   */
  selfWraps?: boolean
  /** Optional child grouping for sidebar section headers (e.g., Settings Core/Advanced). */
  childGroups?: { labelKey: string; tabs: string[] }[]
}

export interface RouteLeaf {
  path: string
  labelKey: string
  component: LazyExoticComponent<ComponentType> | ComponentType
}

// --- Lazy imports: Layouts (pages with children) ---
const DashboardLayout = lazy(() => import('../pages/dashboard/DashboardLayout'))
const SettingsLayout = lazy(() => import('../pages/settings/SettingsLayout'))
const AutomationLayout = lazy(() => import('../pages/automation/AutomationLayout'))
const TimelineLayout = lazy(() => import('../pages/timeline/TimelineLayout'))
const FocusLayout = lazy(() => import('../pages/focus/FocusLayout'))
const ReportsLayout = lazy(() => import('../pages/reports/ReportsLayout'))
const PrivacyLayout = lazy(() => import('../pages/privacy-page/PrivacyLayout'))
const UpdatesLayout = lazy(() => import('../pages/updates/UpdatesLayout'))
const CoachingLayout = lazy(() => import('../pages/coaching/CoachingLayout'))
const RecalibrationLayout = lazy(() => import('../pages/recalibration/RecalibrationLayout'))
const ReplayLayout = lazy(() => import('../pages/session-replay/ReplayLayout'))
const AuditLayout = lazy(() => import('../pages/audit/AuditLayout'))

// --- Lazy imports: Leaf pages (no children) ---
const DashboardDay = lazy(() => import('../pages/DashboardDay'))
const Search = lazy(() => import('../pages/Search'))
const Chat = lazy(() => import('../pages/chat'))
const Policies = lazy(() => import('../pages/policies'))
const Playbooks = lazy(() => import('../pages/Playbooks'))

// --- Lazy imports: Settings sub-routes ---
const GeneralTab = lazy(() => import('../pages/setting-tabs/GeneralTab'))
const PrivacyTab = lazy(() => import('../pages/setting-tabs/PrivacyTab'))
const MonitoringTab = lazy(() => import('../pages/setting-tabs/MonitoringTab'))
const AiAutomationTab = lazy(() => import('../pages/setting-tabs/ai-automation'))
const DataStorageTab = lazy(() => import('../pages/setting-tabs/DataStorageTab'))
const CoachingSettingsTab = lazy(() => import('../pages/setting-tabs/CoachingSettingsTab'))
const SyncTab = lazy(() => import('../pages/setting-tabs/SyncTab'))
const AudioTab = lazy(() => import('../pages/setting-tabs/AudioTab'))
const AdvancedTab = lazy(() => import('../pages/setting-tabs/AdvancedTab'))
const FocusAutoTab = lazy(() => import('../pages/setting-tabs/FocusAutoTab'))
const TrackingScheduleTab = lazy(() =>
  import('../pages/setting-tabs/TrackingScheduleSettings').then((m) => ({ default: m.TrackingScheduleSettings })),
)

// --- Lazy imports: Dashboard sub-routes ---
const OverviewSection = lazy(() => import('../pages/dashboard/OverviewSection'))
const MonitoringSection = lazy(() => import('../pages/dashboard/MonitoringSection'))
const InsightsSection = lazy(() => import('../pages/dashboard/InsightsSection'))

// --- Lazy imports: Automation sub-routes ---
const PoliciesSection = lazy(() => import('../pages/automation/PoliciesSection'))
const CommandsSection = lazy(() => import('../pages/automation/CommandsSection'))
const HistorySection = lazy(() => import('../pages/automation/HistorySection'))

// --- Lazy imports: Timeline sub-routes ---
const AllFrames = lazy(() => import('../pages/timeline/AllFrames'))
const FiltersView = lazy(() => import('../pages/timeline/FiltersView'))

// --- Lazy imports: Focus sub-routes ---
const ScoreSection = lazy(() => import('../pages/focus/ScoreSection'))
const SessionsSection = lazy(() => import('../pages/focus/SessionsSection'))
const InterruptionsSection = lazy(() => import('../pages/focus/InterruptionsSection'))

// --- Lazy imports: Reports sub-routes ---
const ActivityReport = lazy(() => import('../pages/reports/ActivityReport'))
const FocusReport = lazy(() => import('../pages/reports/FocusReport'))
const ExportSection = lazy(() => import('../pages/reports/ExportSection'))

// --- Lazy imports: Privacy sub-routes ---
const DataSection = lazy(() => import('../pages/privacy-page/DataSection'))
const ConsentSection = lazy(() => import('../pages/privacy-page/ConsentSection'))
const PrivacyExportSection = lazy(() => import('../pages/privacy-page/ExportSection'))

// --- Lazy imports: Updates sub-routes ---
const StatusSection = lazy(() => import('../pages/updates/StatusSection'))
const ChannelSection = lazy(() => import('../pages/updates/ChannelSection'))

// --- Lazy imports: Coaching sub-routes ---
const GoalsSection = lazy(() => import('../pages/coaching/GoalsSection'))
const CoachingHistorySection = lazy(() => import('../pages/coaching/HistorySection'))

// --- Lazy imports: Recalibration sub-routes ---
const SegmentsSection = lazy(() => import('../pages/recalibration/SegmentsSection'))
const OverridesSection = lazy(() => import('../pages/recalibration/OverridesSection'))

// --- Lazy imports: Replay sub-routes ---
const ReplayTimeline = lazy(() => import('../pages/session-replay/TimelineSection'))
const EventsSection = lazy(() => import('../pages/session-replay/EventsSection'))

// --- Lazy imports: Audit sub-routes ---
const AuditSummary = lazy(() => import('../pages/audit/SummarySection'))
const AuditEntries = lazy(() => import('../pages/audit/EntriesSection'))

/**
 * Single source of truth for routing, sidebar, and ActivityBar.
 *
 * RouteRenderer auto-generates <Route> elements from this array.
 * SidePanel derives sidebar nodes from `children`.
 * ActivityBar derives nav items from top-level entries.
 *
 * Ordering note: RouteRenderer sorts at render time (leaves first, "/" last),
 * so declaration order here is for readability, not route priority.
 */
export const routeTree: RouteNode[] = [
  // --- Monitor group (real-time observation) ---
  {
    path: '/',
    labelKey: 'nav.dashboard',
    icon: LayoutDashboard,
    defaultChild: 'overview',
    component: DashboardLayout,
    children: [
      { path: 'overview', labelKey: 'sidebar.overview', component: OverviewSection },
      { path: 'monitoring', labelKey: 'sidebar.systemMetrics', component: MonitoringSection },
      { path: 'insights', labelKey: 'sidebar.activityHeatmap', component: InsightsSection },
    ],
    group: 'monitor',
  },
  {
    path: '/day',
    labelKey: 'nav.dashboardDay',
    icon: Calendar,
    component: DashboardDay,
    group: 'monitor',
  },
  {
    path: '/timeline',
    labelKey: 'nav.timeline',
    icon: Clock,
    defaultChild: 'all',
    component: TimelineLayout,
    children: [
      { path: 'all', labelKey: 'sidebar.allFrames', component: AllFrames },
      { path: 'filters', labelKey: 'sidebar.filters', component: FiltersView },
    ],
    group: 'monitor',
  },
  {
    path: '/replay',
    labelKey: 'nav.replay',
    icon: Zap,
    defaultChild: 'timeline',
    component: ReplayLayout,
    children: [
      { path: 'timeline', labelKey: 'sidebar.timeline', component: ReplayTimeline },
      { path: 'events', labelKey: 'sidebar.eventLog', component: EventsSection },
    ],
    group: 'monitor',
  },
  {
    path: '/focus',
    labelKey: 'nav.focus',
    icon: Image,
    defaultChild: 'score',
    component: FocusLayout,
    children: [
      { path: 'score', labelKey: 'sidebar.currentScore', component: ScoreSection },
      { path: 'sessions', labelKey: 'sidebar.focusSessions', component: SessionsSection },
      { path: 'interruptions', labelKey: 'sidebar.interruptions', component: InterruptionsSection },
    ],
    group: 'monitor',
  },

  // --- Insights group (analysis & AI) ---
  {
    path: '/reports',
    labelKey: 'nav.reports',
    icon: BarChart3,
    defaultChild: 'activity',
    component: ReportsLayout,
    children: [
      { path: 'activity', labelKey: 'sidebar.activityReport', component: ActivityReport },
      { path: 'focus', labelKey: 'sidebar.focusReport', component: FocusReport },
      { path: 'export', labelKey: 'sidebar.exportData', component: ExportSection },
    ],
    group: 'insights',
  },
  {
    path: '/coaching',
    labelKey: 'nav.coaching',
    icon: MessageCircle,
    defaultChild: 'goals',
    component: CoachingLayout,
    children: [
      { path: 'goals', labelKey: 'sidebar.coachingGoals', component: GoalsSection },
      { path: 'history', labelKey: 'sidebar.coachingEvents', component: CoachingHistorySection },
    ],
    group: 'insights',
  },
  {
    path: '/chat',
    labelKey: 'nav.chat',
    icon: MessageSquare,
    component: Chat,
    group: 'insights',
  },
  {
    path: '/playbooks',
    labelKey: 'nav.playbooks',
    icon: BookOpen,
    component: Playbooks,
    group: 'insights',
  },
  {
    path: '/search',
    labelKey: 'nav.search',
    icon: Tag,
    component: Search,
    group: 'insights',
  },

  // --- Manage group (control & administration) ---
  {
    path: '/automation',
    labelKey: 'nav.automation',
    icon: Monitor,
    defaultChild: 'policies',
    component: AutomationLayout,
    children: [
      { path: 'policies', labelKey: 'sidebar.policies', component: PoliciesSection },
      { path: 'commands', labelKey: 'sidebar.commands', component: CommandsSection },
      { path: 'history', labelKey: 'sidebar.executionHistory', component: HistorySection },
    ],
    group: 'manage',
  },
  {
    path: '/recalibration',
    labelKey: 'nav.recalibration',
    icon: RefreshCw,
    defaultChild: 'segments',
    component: RecalibrationLayout,
    children: [
      { path: 'segments', labelKey: 'sidebar.segments', component: SegmentsSection },
      { path: 'overrides', labelKey: 'sidebar.overrideHistory', component: OverridesSection },
    ],
    group: 'manage',
  },
  {
    path: '/policies',
    labelKey: 'nav.policies',
    icon: Shield,
    component: Policies,
    group: 'manage',
  },
  {
    path: '/audit',
    labelKey: 'nav.audit',
    icon: ClipboardList,
    defaultChild: 'summary',
    component: AuditLayout,
    children: [
      { path: 'summary', labelKey: 'sidebar.auditSummary', component: AuditSummary },
      { path: 'entries', labelKey: 'sidebar.auditEntries', component: AuditEntries },
    ],
    group: 'manage',
  },
  {
    path: '/updates',
    labelKey: 'nav.updates',
    icon: FileText,
    defaultChild: 'status',
    component: UpdatesLayout,
    children: [
      { path: 'status', labelKey: 'sidebar.currentStatus', component: StatusSection },
      { path: 'channel', labelKey: 'sidebar.updateHistory', component: ChannelSection },
    ],
    group: 'manage',
  },

  // --- Bottom items ---
  {
    path: '/settings',
    labelKey: 'nav.settings',
    icon: Settings,
    defaultChild: 'general',
    component: SettingsLayout,
    // SettingsLayout wraps its own RouteErrorBoundary so that
    // SettingsFormProvider lives ABOVE the boundary. Without this, a recovery
    // reset would remount the provider and silently destroy unsaved form edits.
    selfWraps: true,
    childGroups: [
      { labelKey: 'settings.groupCore', tabs: ['general', 'privacy', 'monitoring', 'coaching', 'audio'] },
      {
        labelKey: 'settings.groupAdvanced',
        tabs: ['ai-automation', 'data', 'sync', 'focus-auto', 'advanced', 'tracking-schedule'],
      },
    ],
    children: [
      // Core group
      { path: 'general', labelKey: 'settings.tabs.general', component: GeneralTab },
      { path: 'privacy', labelKey: 'settings.tabs.privacy', component: PrivacyTab },
      { path: 'monitoring', labelKey: 'settings.tabs.monitoring', component: MonitoringTab },
      { path: 'coaching', labelKey: 'settings.tabs.coaching', component: CoachingSettingsTab },
      { path: 'audio', labelKey: 'settings.tabs.audio', component: AudioTab },
      // Advanced group
      { path: 'ai-automation', labelKey: 'settings.tabs.aiAutomation', component: AiAutomationTab },
      { path: 'data', labelKey: 'settings.tabs.dataStorage', component: DataStorageTab },
      { path: 'sync', labelKey: 'settings.tabs.sync', component: SyncTab },
      { path: 'focus-auto', labelKey: 'settings.tabs.focusAuto', component: FocusAutoTab },
      { path: 'advanced', labelKey: 'settings.tabs.advanced', component: AdvancedTab },
      { path: 'tracking-schedule', labelKey: 'settings.tabs.trackingSchedule', component: TrackingScheduleTab },
    ],
    bottom: true,
  },
  {
    path: '/privacy',
    labelKey: 'nav.privacy',
    icon: Info,
    defaultChild: 'data',
    component: PrivacyLayout,
    children: [
      { path: 'data', labelKey: 'sidebar.dataControls', component: DataSection },
      { path: 'consent', labelKey: 'sidebar.dangerZone', component: ConsentSection },
      { path: 'export', labelKey: 'sidebar.dataExport', component: PrivacyExportSection },
    ],
    bottom: true,
  },
]

// ---------------------------------------------------------------------------
// Top-level category navigation
// ---------------------------------------------------------------------------
//
// The ActivityBar renders a small fixed set of category icons rather than one
// icon per top-level route.  Clicking a category icon navigates to its
// `defaultPath` and the SidePanel expands to show the entire group's routes
// (top-level + their `children`) as a nested tree.  This keeps the 48px rail
// uncluttered while still making every route reachable in two clicks.

export type NavGroupId = 'monitor' | 'insights' | 'manage'

export interface NavGroup {
  id: NavGroupId
  labelKey: string
  icon: ComponentType<{ className?: string }>
  /**
   * Where the group icon navigates to when it's not already active.
   * Should correspond to a route whose `group` field matches `id`.
   */
  defaultPath: string
}

export const navGroups: NavGroup[] = [
  { id: 'monitor', labelKey: 'nav.groupMonitor', icon: Gauge, defaultPath: '/' },
  // /reports is the Insights landing because it surfaces charts and activity
  // summaries that give users the broadest overview of their analysis data.
  { id: 'insights', labelKey: 'nav.groupInsights', icon: Lightbulb, defaultPath: '/reports' },
  { id: 'manage', labelKey: 'nav.groupManage', icon: Wrench, defaultPath: '/automation' },
]

/**
 * Return every top-level route that belongs to a nav group, preserving the
 * declaration order in `routeTree`.  Used by `SidePanel` (group mode) to
 * build the tree shown in the resizable panel.
 *
 * @param group The nav group ID — `'monitor' | 'insights' | 'manage'`.
 * @returns The routes whose `group` field matches. Empty if the group has
 *          no routes (e.g. `manage` during an IA experiment), in which case
 *          the SidePanel falls back to rendering `null`.
 */
export function getRoutesForGroup(group: NavGroupId): RouteNode[] {
  return routeTree.filter((r) => r.group === group)
}

/**
 * Build the full child-qualified path for a nested route leaf.
 * e.g. (`/`, `overview`) → `/overview`; (`/timeline`, `all`) → `/timeline/all`.
 */
export function joinChildPath(parent: RouteNode, child: RouteLeaf): string {
  if (parent.path === '/') return `/${child.path}`
  return `${parent.path}/${child.path}`
}
