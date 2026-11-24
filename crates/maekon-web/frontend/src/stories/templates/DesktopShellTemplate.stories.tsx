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
 * Main tree (#13736): landing on /overview activates the **Menu** group icon
 * and the SidePanel renders six front destinations followed by the folded
 * More / Tools / System bundles.  Use this story to verify the tree density
 * and selection highlight.
 */
export const MainTreeExperience: Story = {
  args: {
    route: '/overview',
    title: 'Main tree — /overview',
    description:
      'Clicking Menu lands here and opens the main tree. Dashboard.Overview is the highlighted leaf, Dashboard is the expanded parent, and the More / Tools / System bundles stay folded.',
    checklistItems: [
      'Menu icon shows active indicator bar.',
      'SidePanel header reads "Menu".',
      'Dashboard tree node is expanded and Overview is selected.',
      'Exactly six destinations (Dashboard, Timeline, Search, Focus, Reports, Chat) sit at the first level, followed by the More, Tools and System bundles.',
    ],
  },
}

/**
 * Deep-link into /automation/policies: the main group stays active and the
 * SidePanel opens the Tools bundle to highlight the nested Runtime Status
 * leaf under the Automation parent.  Verifies that three-level nesting
 * renders and selection propagates.
 */
export const NestedChildExperience: Story = {
  args: {
    route: '/automation/policies',
    title: 'Nested child — /automation/policies',
    description:
      'The main group remains active because /automation belongs to it. The SidePanel opens the Tools bundle and highlights the Runtime Status leaf nested under Automation.',
    checklistItems: [
      'Menu icon still shows the active indicator (not Automation as its own icon).',
      'Tools bundle and Automation parent are expanded and Runtime Status is selected.',
      'The six front destinations stay visible but unselected; More and System stay folded.',
    ],
  },
}

/**
 * Front destination with children (/reports/activity).  Verifies that only
 * the Reports parent opens while the bundles stay folded.
 */
export const FrontDestinationExperience: Story = {
  args: {
    route: '/reports/activity',
    title: 'Front destination — /reports/activity',
    description:
      'Reports is one of the six front destinations. The SidePanel renders the main tree with Reports expanded and Activity Report selected.',
    checklistItems: [
      'Menu icon shows the active indicator.',
      'Reports tree node is expanded and Activity Report is selected.',
      'More, Tools and System bundles are folded.',
    ],
  },
}

/**
 * Bundled route (/audit/summary).  The Tools bundle opens to reveal the
 * current page while the other bundles stay folded.
 */
export const BundledRouteExperience: Story = {
  args: {
    route: '/audit/summary',
    title: 'Bundled route — /audit/summary',
    description:
      'Audit Log sits one level back in the Tools bundle. Use this story to check that an opened bundle still reads as secondary to the front destinations.',
    checklistItems: [
      'Menu icon shows the active indicator.',
      'Tools is expanded, Audit Log is expanded, and Summary is selected.',
      'Execution Policies is a leaf (no chevron).',
      'More and System stay folded.',
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
      'Settings does not belong to any nav group, so the SidePanel falls back to the legacy "children of current route" rendering — Settings sub-tabs only, nothing from the main tree.',
    checklistItems: [
      'Settings icon shows the active indicator.',
      'SidePanel header reads "Settings" (not a group label).',
      'Only Settings sub-tabs are listed (no main-tree cross-pollination).',
      'Monitoring sub-tab is selected.',
    ],
  },
}
