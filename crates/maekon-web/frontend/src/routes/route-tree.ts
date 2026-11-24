import {
  BarChart3,
  BookOpen,
  Calendar,
  CalendarRange,
  CircleArrowUp,
  ClipboardList,
  Clock,
  Cpu,
  FileSpreadsheet,
  Home,
  LayoutDashboard,
  LifeBuoy,
  ListChecks,
  LogIn,
  Mail,
  Menu,
  MessageCircle,
  MessageSquare,
  RefreshCw,
  ScanSearch,
  ScrollText,
  Settings,
  ShieldCheck,
  Target,
  Workflow,
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
  group?: NavGroupId
  /**
   * Bundle that holds this route one level back in the main side-panel tree.
   * Group routes without a `section` are the front destinations at the first
   * tree level (see `navSections` and `getGroupLayout`).
   */
  section?: NavSectionId
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
const WbsXlsxDraftPage = lazy(() => import('../pages/wbs-xlsx-draft/WbsXlsxDraftPage'))

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
 * SidePanel derives the main tree from `group` + `section`, and the
 * Settings / Privacy trees from `children`.
 * ActivityBar derives its group icon from `navGroups` and its bottom icons
 * from `bottom`.
 *
 * #13736: routes in the `main` group without a `section` are the front
 * destinations at the first side-panel level; every other group route sits one
 * level back under a `navSections` bundle. No path changed in that move, so old
 * URLs, deep links, notification targets and CommandPalette entries keep
 * working — `routes/__tests__/legacyPaths.test.tsx` freezes the earlier list.
 *
 * Ordering note: RouteRenderer sorts at render time (leaves first, "/" last),
 * so declaration order here is the display order of the side panel and the
 * command palette, not route priority.
 */
export const routeTree: RouteNode[] = [
  // --- Front destinations (first side-panel level) ---
  {
    path: '/',
    labelKey: 'nav.dashboard',
    icon: LayoutDashboard,
    defaultChild: 'overview',
    component: DashboardLayout,
    children: [
      { path: 'overview', labelKey: 'sidebar.overview', component: OverviewSection },
      { path: 'insights', labelKey: 'sidebar.activityHeatmap', component: InsightsSection },
    ],
    group: 'main',
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
    group: 'main',
  },
  {
    path: '/search',
    labelKey: 'nav.search',
    icon: ScanSearch,
    component: Search,
    group: 'main',
  },
  {
    path: '/focus',
    labelKey: 'nav.focus',
    icon: Target,
    defaultChild: 'score',
    component: FocusLayout,
    children: [
      { path: 'score', labelKey: 'sidebar.currentScore', component: ScoreSection },
      { path: 'sessions', labelKey: 'sidebar.focusSessions', component: SessionsSection },
      { path: 'interruptions', labelKey: 'sidebar.interruptions', component: InterruptionsSection },
    ],
    group: 'main',
  },
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
    group: 'main',
  },
  {
    path: '/chat',
    labelKey: 'nav.chat',
    icon: MessageSquare,
    component: Chat,
    group: 'main',
  },

  // --- "More" bundle (periodic review surfaces) ---
  {
    path: '/day',
    labelKey: 'nav.dashboardDay',
    icon: Calendar,
    component: DashboardDay,
    group: 'main',
    section: 'more',
  },
  {
    path: '/week',
    labelKey: 'nav.dashboardWeek',
    icon: CalendarRange,
    component: DashboardWeek,
    group: 'main',
    section: 'more',
  },
  {
    path: '/replay',
    labelKey: 'nav.replay',
    icon: Zap,
    defaultChild: 'timeline',
    component: ReplayLayout,
    children: [
      { path: 'timeline', labelKey: 'sidebar.playback', component: ReplayTimeline },
      { path: 'events', labelKey: 'sidebar.eventLog', component: EventsSection },
    ],
    group: 'main',
    section: 'more',
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
    group: 'main',
    section: 'more',
  },
  {
    path: '/tasks',
    labelKey: 'nav.tasks',
    icon: ListChecks,
    component: TasksPage,
    group: 'main',
    section: 'more',
  },

  // --- "Tools" bundle (automation and the controls around it) ---
  {
    path: '/automation',
    labelKey: 'nav.automation',
    icon: Workflow,
    defaultChild: 'policies',
    component: AutomationLayout,
    children: [
      { path: 'policies', labelKey: 'sidebar.policies', component: PoliciesSection },
      { path: 'commands', labelKey: 'sidebar.commands', component: CommandsSection },
      { path: 'history', labelKey: 'sidebar.executionHistory', component: HistorySection },
    ],
    group: 'main',
    section: 'tools',
  },
  {
    path: '/playbooks',
    labelKey: 'nav.playbooks',
    icon: BookOpen,
    component: Playbooks,
    group: 'main',
    section: 'tools',
  },
  {
    path: '/policies',
    labelKey: 'nav.policies',
    icon: ScrollText,
    component: Policies,
    group: 'main',
    section: 'tools',
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
    group: 'main',
    section: 'tools',
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
    group: 'main',
    section: 'tools',
  },

  // --- "System" bundle (app health and help) ---
  {
    path: '/updates',
    labelKey: 'nav.updates',
    icon: CircleArrowUp,
    defaultChild: 'status',
    component: UpdatesLayout,
    children: [
      { path: 'status', labelKey: 'sidebar.currentStatus', component: StatusSection },
      { path: 'channel', labelKey: 'sidebar.updateChannel', component: ChannelSection },
    ],
    group: 'main',
    section: 'system',
  },
  // #8079: dedicated Support & Diagnostics destination. #13736 moved it from a
  // bottom rail icon into the System bundle; the path did not change.
  {
    path: '/support',
    labelKey: 'nav.support',
    icon: LifeBuoy,
    component: SupportPage,
    group: 'main',
    section: 'system',
  },
  // #13735 moved CPU, memory and process data off the Dashboard's first screen
  // to this leaf at the former Dashboard child URL, so deep links keep working.
  // #13736 puts it in the System bundle next to Support, which links to it.
  {
    path: '/monitoring',
    labelKey: 'sidebar.systemMetrics',
    icon: Cpu,
    component: MonitoringSection,
    group: 'main',
    section: 'system',
  },

  // --- Bottom items (direct ActivityBar icons) ---
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
    icon: ShieldCheck,
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
  // #9603 WD-02.1: the connected-mode sign-in screen. Deliberately neither
  // `group` nor `bottom`, so it claims no slot in the side-panel tree or on the
  // 48px ActivityBar rail —
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
  // Like `/login`: no `group` and no `bottom`, so it claims no slot in the side
  // panel or on the 48px rail — but it carries an `icon` so CommandPalette
  // lists it and it is reachable by name rather than only as a post-login
  // redirect target.
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
  // #11120 WD-04.5: receipt-only native XLSX picker, writer, and receipt surface.
  {
    path: '/wbs-xlsx-draft',
    labelKey: 'nav.wbsXlsxDraft',
    icon: FileSpreadsheet,
    component: WbsXlsxDraftPage,
  },
]

// ---------------------------------------------------------------------------
// Side-panel navigation: one group, front destinations, and bundles
// ---------------------------------------------------------------------------
//
// #13736: the rail used to carry three category icons (Monitor / Insights /
// Manage) whose trees listed 17 top-level destinations between them, next to
// three bottom icons. A newcomer had 20 places to choose from. Now the single
// `main` group icon opens one tree: the six front destinations (group routes
// without a `section`) at the first level, and every other group route one
// level back under a `navSections` bundle. Settings and Privacy stay as direct
// bottom icons, so 6 + 2 destinations are visible up front.

export type NavGroupId = 'main'

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

export const navGroups: NavGroup[] = [{ id: 'main', labelKey: 'nav.groupMain', icon: Menu, defaultPath: '/' }]

export type NavSectionId = 'more' | 'tools' | 'system'

export interface NavSection {
  id: NavSectionId
  labelKey: string
}

/** Bundles one level back in the main tree, in display order. */
export const navSections: NavSection[] = [
  { id: 'more', labelKey: 'nav.sectionMore' },
  { id: 'tools', labelKey: 'nav.sectionTools' },
  { id: 'system', labelKey: 'nav.sectionSystem' },
]

/**
 * Return every top-level route that belongs to a nav group, preserving the
 * declaration order in `routeTree`.
 *
 * @param group The nav group ID.
 * @returns The routes whose `group` field matches. Empty if the group has
 *          no routes, in which case the SidePanel falls back to rendering
 *          `null`.
 */
export function getRoutesForGroup(group: NavGroupId): RouteNode[] {
  return routeTree.filter((r) => r.group === group)
}

export interface GroupLayout {
  /** Routes shown at the first tree level, in declaration order. */
  front: RouteNode[]
  /** Non-empty bundles in `navSections` order, each with its routes. */
  sections: { section: NavSection; routes: RouteNode[] }[]
}

/**
 * Split a group's routes into the front destinations and the bundles one
 * level back. Used by `SidePanel` to build the main tree and by the IA budget
 * test, so both read the same layout.
 */
export function getGroupLayout(group: NavGroupId): GroupLayout {
  const routes = getRoutesForGroup(group)
  return {
    front: routes.filter((r) => !r.section),
    sections: navSections
      .map((section) => ({ section, routes: routes.filter((r) => r.section === section.id) }))
      .filter((entry) => entry.routes.length > 0),
  }
}

/**
 * Build the full child-qualified path for a nested route leaf.
 * e.g. (`/`, `overview`) → `/overview`; (`/timeline`, `all`) → `/timeline/all`.
 */
export function joinChildPath(parent: RouteNode, child: RouteLeaf): string {
  if (parent.path === '/') return `/${child.path}`
  return `${parent.path}/${child.path}`
}
