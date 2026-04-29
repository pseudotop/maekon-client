import type { Meta, StoryObj } from '@storybook/react'
import { Badge, Card, CardTitle } from '../../components/ui'
import {
  darkThemeGlobals,
  lightThemeGlobals,
  ReviewHeader,
  ReviewNote,
  reviewStoryParameters,
  ShellStoryFrame,
} from '../storybook-helpers'

interface DesktopShellTemplateProps {
  route?: string
  title?: string
  description?: string
  checklistItems?: string[]
}

function DesktopShellTemplate({
  route = '/settings/monitoring',
  title = 'Desktop Shell',
  description = 'Shell chrome, side navigation, and content spacing review artifact for route-level contrast and density checks.',
  checklistItems = [
    'Centered title stays legible in light theme.',
    'Activity bar active state remains visible without oversaturation.',
    'Side panel header and tree labels keep contrast on both themes.',
    'Status bar text remains readable against the brand bar.',
  ],
}: DesktopShellTemplateProps = {}) {
  return (
    <ShellStoryFrame route={route} contentClassName="min-h-[calc(100vh-3.5rem)]">
      <div className="space-y-6 p-6">
        <ReviewHeader eyebrow="Template Review" title={title} description={description} />

        <ReviewNote>
          Use this template to review shell alignment, page-title contrast, left-rail emphasis, and status-bar
          readability before checking a specific route page.
        </ReviewNote>

        <div className="grid gap-6 xl:grid-cols-[1.7fr_1fr]">
          <Card variant="default" padding="lg">
            <div className="mb-4 flex items-center justify-between">
              <CardTitle>Monitoring readiness</CardTitle>
              <Badge color="warning">Permission follow-up</Badge>
            </div>
            <div className="space-y-3 text-content-secondary text-sm">
              <p>
                Accessibility is granted, screen recording still needs attention, and notifications remain optional.
              </p>
              <p>
                This frame is intentionally content-heavy enough to expose weak heading contrast, muted text problems,
                and overly flat card groupings.
              </p>
            </div>
          </Card>

          <Card variant="elevated" padding="lg">
            <CardTitle className="mb-4">Review checklist</CardTitle>
            <ul className="space-y-2 text-content-secondary text-sm">
              {checklistItems.map((item) => (
                <li key={item}>{item}</li>
              ))}
            </ul>
          </Card>
        </div>
      </div>
    </ShellStoryFrame>
  )
}

const meta = {
  title: 'Templates/DesktopShell',
  component: DesktopShellTemplate,
  tags: ['autodocs'],
  parameters: reviewStoryParameters,
} satisfies Meta<typeof DesktopShellTemplate>

export default meta
type Story = StoryObj<typeof meta>

export const Default: Story = {}

export const LightReview: Story = {
  globals: lightThemeGlobals,
}

export const DarkReview: Story = {
  globals: darkThemeGlobals,
}

/**
 * Showcase for the v0.4.32 category restructure: landing on /overview should
 * activate the **Monitor** group icon and the SidePanel should render the
 * entire Monitor tree (Dashboard, Day, Timeline, Replay, Automation) with
 * first-level items auto-expanded.  Use this story to verify the group tree
 * density and selection highlight.
 */
export const MonitorGroupExperience: Story = {
  args: {
    route: '/overview',
    title: 'Monitor group — /overview',
    description:
      'Clicking Monitor lands here and opens the Monitor tree. Dashboard.Overview is the highlighted leaf, Dashboard is the expanded parent.',
    checklistItems: [
      'Monitor icon shows active indicator bar.',
      'SidePanel header reads "Monitor".',
      'Dashboard tree node is expanded and Overview is selected.',
      'Every other group route (Day, Timeline, Replay, Automation) is visible as a top-level treeitem.',
    ],
  },
}

/**
 * Deep-link into /automation/policies: the Monitor group stays active and
 * the SidePanel highlights the nested Policies leaf under the Automation
 * parent.  Verifies that two-level nesting renders + selection propagates.
 */
export const NestedChildExperience: Story = {
  args: {
    route: '/automation/policies',
    title: 'Nested child — /automation/policies',
    description:
      'The Monitor group remains active because /automation belongs to the monitor group. The SidePanel tree highlights the Policies leaf nested under Automation.',
    checklistItems: [
      'Monitor icon still shows the active indicator (not Automation as its own icon).',
      'Automation parent is expanded and Policies leaf is selected.',
      'Dashboard/Day/Timeline/Replay remain visible but unselected.',
    ],
  },
}

/**
 * Data group landing view (/reports/activity).  Verifies that the Data group
 * tree renders every data-group route (Recalibration, Coaching, Playbooks,
 * Chat, Focus, Reports, Search) with Reports.Activity selected.
 */
export const DataGroupExperience: Story = {
  args: {
    route: '/reports/activity',
    title: 'Data group — /reports/activity',
    description:
      'Clicking Data lands on the reports activity view. The SidePanel renders the full Data tree with Reports expanded and Activity selected.',
    checklistItems: [
      'Data icon shows the active indicator.',
      'Reports tree node is expanded and Activity Report is selected.',
      'Every data-group leaf (Recalibration, Coaching, Playbooks, Chat, Focus, Search) is visible.',
    ],
  },
}

/**
 * Manage group (/audit/summary).  Smaller group (only 3 top-level entries)
 * so the tree should feel airy with room to spare in the resizable panel.
 */
export const ManageGroupExperience: Story = {
  args: {
    route: '/audit/summary',
    title: 'Manage group — /audit/summary',
    description:
      'The Manage group only has three top-level entries (Audit, Policies, Updates). Use this story to check that a sparse group tree still feels intentional rather than empty.',
    checklistItems: [
      'Manage icon shows the active indicator.',
      'Audit is expanded with Summary selected.',
      'Policies is a leaf (no chevron).',
      'Updates is collapsed by default.',
    ],
  },
}

/**
 * Bottom-route legacy mode: /settings shows only Settings children (not the
 * full group tree).  Use this to verify the fallback rendering path remains
 * intact for Settings and Privacy.
 */
export const SettingsBottomMode: Story = {
  args: {
    route: '/settings/monitoring',
    title: 'Settings — legacy bottom mode',
    description:
      'Settings does not belong to any nav group, so the SidePanel falls back to the legacy "children of current route" rendering — nine Settings sub-tabs only, nothing from Monitor/Data/Manage.',
    checklistItems: [
      'Settings icon shows the active indicator.',
      'SidePanel header reads "Settings" (not a group label).',
      'Only Settings sub-tabs are listed (no Monitor/Data/Manage cross-pollination).',
      'Monitoring sub-tab is selected.',
    ],
  },
}
