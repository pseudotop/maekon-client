import {
  BarChart3,
  BookOpen,
  Calendar,
  CalendarRange,
  ClipboardList,
  Clock,
  FileText,
  Gauge,
  Home,
  Image,
  Info,
  LayoutDashboard,
  LifeBuoy,
  Lightbulb,
  ListChecks,
  LogIn,
  Mail,
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
import { withCaptureReauthGate } from '../components/CaptureReauthGate'

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
// #8044: capture-history surfaces are wrapped in the re-auth gate — the
// layout stays unmounted until authenticated, so frame fetches never happen
// prematurely (the backend 403 is the backstop).
const TimelineLayout = withCaptureReauthGate(lazy(() => import('../pages/timeline/TimelineLayout')))
const FocusLayout = lazy(() => import('../pages/focus/FocusLayout'))
const ReportsLayout = lazy(() => import('../pages/reports/ReportsLayout'))
const PrivacyLayout = lazy(() => import('../pages/privacy-page/PrivacyLayout'))
const UpdatesLayout = lazy(() => import('../pages/updates/UpdatesLayout'))
const CoachingLayout = lazy(() => import('../pages/coaching/CoachingLayout'))
const RecalibrationLayout = lazy(() => import('../pages/recalibration/RecalibrationLayout'))
const ReplayLayout = withCaptureReauthGate(lazy(() => import('../pages/session-replay/ReplayLayout')))
const AuditLayout = lazy(() => import('../pages/audit/AuditLayout'))

// --- Lazy imports: Leaf pages (no children) ---
const DashboardDay = lazy(() => import('../pages/DashboardDay'))
const DashboardWeek = lazy(() => import('../pages/DashboardWeek'))
// Search reads capture history through the backend `/api/search` endpoint, so
// it must use the same re-auth gate as timeline and replay. Without this
// wrapper, the expected `auth.reauth_required` backstop is rendered as a
// generic search failure instead of an unlock prompt.
const Search = withCaptureReauthGate(lazy(() => import('../pages/Search')))
const Chat = lazy(() => import('../pages/chat'))
const Policies = lazy(() => import('../pages/policies'))
const Playbooks = lazy(() => import('../pages/Playbooks'))
const TasksPage = lazy(() => import('../pages/tasks/TasksPage'))
const SupportPage = lazy(() => import('../pages/support/SupportPage'))
const LoginPage = lazy(() => import('../pages/auth/LoginPage'))
const ContextHomePage = lazy(() => import('../pages/home/ContextHomePage'))
const AssignmentEmailDraftPage = lazy(() => import('../pages/assignment-email-draft/AssignmentEmailDraftPage'))

// --- Lazy imports: Settings sub-routes ---
const GeneralTab = lazy(() => import('../pages/setting-tabs/GeneralTab'))
const PrivacyTab = lazy(() => import('../pages/setting-tabs/PrivacyTab'))
const MonitoringTab = lazy(() => import('../pages/setting-tabs/MonitoringTab'))
const AiAutomationTab = lazy(() => import('../pages/setting-tabs/ai-automation'))
const DataStorageTab = lazy(() => import('../pages/setting-tabs/DataStorageTab'))
const CoachingSettingsTab = lazy(() => import('../pages/setting-tabs/CoachingSettingsTab'))
const SyncTab = lazy(() => import('../pages/setting-tabs/SyncTab'))
const IntegrationsTab = lazy(() => import('../pages/setting-tabs/IntegrationsTab'))
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
// Backup downloads can include capture OCR/window metadata, while the GDPR
// full export contains the complete personal-data archive. Gate the export
// leaf without blocking the other privacy controls.
const PrivacyExportSection = withCaptureReauthGate(lazy(() => import('../pages/privacy-page/ExportSection')))
const EgressLedgerSection = lazy(() => import('../pages/privacy-page/EgressLedgerSection'))
const ClaimsSection = lazy(() => import('../pages/privacy-page/ClaimsSection'))

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
    path: '/week',
    labelKey: 'nav.dashboardWeek',
    icon: CalendarRange,
    component: DashboardWeek,
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
    path: '/tasks',
    labelKey: 'nav.tasks',
    icon: ListChecks,
    component: TasksPage,
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
      { path: 'channel', labelKey: 'sidebar.updateChannel', component: ChannelSection },
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
    // SettingsLayout wraps its own RouteErrorBoundary. SettingsFormProvider
    // lives above the app route renderer so recovery and top-level navigation
    // cannot silently destroy unsaved form edits.
    selfWraps: true,
    childGroups: [
      { labelKey: 'settings.groupCore', tabs: ['general', 'privacy', 'monitoring', 'coaching', 'audio'] },
      {
        labelKey: 'settings.groupAdvanced',
        tabs: ['ai-automation', 'data', 'sync', 'integrations', 'focus-auto', 'advanced', 'tracking-schedule'],
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
      { path: 'integrations', labelKey: 'settings.tabs.integrations', component: IntegrationsTab },
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
      { path: 'egress', labelKey: 'sidebar.egressLedger', component: EgressLedgerSection },
      { path: 'claims', labelKey: 'sidebar.claimsBrowser', component: ClaimsSection },
      { path: 'consent', labelKey: 'sidebar.dangerZone', component: ConsentSection },
      { path: 'export', labelKey: 'sidebar.dataExport', component: PrivacyExportSection },
    ],
    bottom: true,
  },
  // #8079: dedicated Support & Diagnostics destination — a childless bottom
  // leaf (like a group leaf, it renders no SidePanel tree; the ActivityBar
  // icon navigates directly to the page).
  {
    path: '/support',
    labelKey: 'nav.support',
    icon: LifeBuoy,
    component: SupportPage,
    bottom: true,
  },
  // #9603 WD-02.1: the connected-mode sign-in screen. Deliberately neither
  // `group` nor `bottom`, so it claims no slot on the 48px ActivityBar rail —
  // signing in is optional in a local-first product and does not deserve
  // permanent nav chrome, least of all in the default build where connected
  // mode is not compiled in at all. It is still a first-class destination:
  // `icon` is what makes CommandPalette list it (buildNavigationItems skips
  // iconless nodes), so it is reachable by name rather than only by deep link.
  //
  // It renders inside the AppShell like every other route; the page's own
  // four-state render is what handles "this build has no connected mode".
  {
    path: '/login',
    labelKey: 'nav.signIn',
    icon: LogIn,
    component: LoginPage,
  },
  // #9611 WD-02.3: where a signed-in operator lands — their own mail threads,
  // messenger history and projects, from the single #9625 typed IPC call.
  //
  // A sibling of `/`, not a replacement for it. `/` is the local-first activity
  // dashboard that works with no account at all; this route only has content
  // once connected mode is compiled in AND signed in. Redirecting `/` here would
  // make the default build's landing page an error state, so the two coexist and
  // sign-in is what moves you between them.
  //
  // Like `/login`: no `group` and no `bottom`, so it claims no slot on the 48px
  // rail — but it carries an `icon` so CommandPalette lists it and it is
  // reachable by name rather than only as a post-login redirect target.
  {
    path: '/home',
    labelKey: 'nav.contextHome',
    icon: Home,
    component: ContextHomePage,
  },
  // #9627 WD-04.4: receipt-only local editor and explicit OS compose handoff.
  // It stays off the permanent rail but remains command-palette reachable.
  {
    path: '/assignment-email-draft',
    labelKey: 'nav.assignmentEmailDraft',
    icon: Mail,
    component: AssignmentEmailDraftPage,
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
