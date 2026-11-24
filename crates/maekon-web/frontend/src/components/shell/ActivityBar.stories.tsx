import type { Meta, StoryObj } from '@storybook/react'
import { AppMemoryRouter } from '../../router/future'
import ActivityBar from './ActivityBar'

const meta = {
  title: 'Shell/ActivityBar',
  component: ActivityBar,
  tags: ['autodocs'],
  parameters: {
    docs: {
      description: {
        component: `
Since #13736 the ActivityBar exposes **three** buttons in the 48px rail:

- 1 main group icon (**Menu**) that lands on the dashboard and opens the
  SidePanel's main tree: six front destinations plus the More / Tools /
  System bundles one level back.
- 2 bottom direct icons (**Settings** / **Privacy**) — high-traffic entry
  points that would be annoying to hide behind an extra click.

Clicking the already-active group icon toggles the SidePanel (VS Code-style)
so users can reclaim screen width without reaching for \`Cmd/Ctrl+B\`.
`.trim(),
      },
    },
  },
  decorators: [
    (Story) => (
      <div style={{ height: 600 }} className="flex">
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof ActivityBar>

export default meta
type Story = StoryObj<typeof meta>

const routeDecorator = (route: string): Meta<typeof ActivityBar>['decorators'] => [
  (Story) => (
    <AppMemoryRouter initialEntries={[route]}>
      <div style={{ height: 600 }} className="flex">
        <Story />
      </div>
    </AppMemoryRouter>
  ),
]

const noOp = () => {}

/** Main group active (landing on /overview after clicking Menu). */
export const MainGroupActive: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: false },
  decorators: routeDecorator('/overview'),
}

/** A front destination (/reports/activity) keeps the main group icon active. */
export const FrontDestinationActive: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: false },
  decorators: routeDecorator('/reports/activity'),
}

/** A route inside the Tools bundle (/audit/summary) keeps the main group icon active. */
export const BundledRouteActive: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: false },
  decorators: routeDecorator('/audit/summary'),
}

/** Bottom direct icon (Settings) active. */
export const SettingsActive: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: false },
  decorators: routeDecorator('/settings/general'),
}

/** Bottom direct icon (Privacy) active. */
export const PrivacyActive: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: false },
  decorators: routeDecorator('/privacy/data'),
}

/** Nested route /automation/policies still highlights the main group. */
export const NestedRouteHighlightsGroup: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: false },
  decorators: routeDecorator('/automation/policies'),
}

/** Collapsed sidebar: the active indicator still renders on the group icon. */
export const SidebarCollapsed: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: true },
  decorators: routeDecorator('/overview'),
}
