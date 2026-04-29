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
After the v0.4.32 category restructure, the ActivityBar only exposes **five**
buttons in the 48px rail:

- 3 category icons (**Monitor** / **Data** / **Manage**) that navigate to the
  first route in their group and open the SidePanel with the group tree.
- 2 bottom direct icons (**Settings** / **Privacy**) — high-traffic entry
  points that would be annoying to hide behind an extra click.

Clicking the already-active category toggles the SidePanel (VS Code-style)
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

/** Monitor group active (landing on /overview after clicking Monitor). */
export const MonitorGroupActive: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: false },
  decorators: routeDecorator('/overview'),
}

/** Data group active — reached from /reports/activity. */
export const DataGroupActive: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: false },
  decorators: routeDecorator('/reports/activity'),
}

/** Manage group active — /audit/summary triggers the manage icon highlight. */
export const ManageGroupActive: Story = {
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

/** Nested route /automation/policies still highlights the Monitor group. */
export const NestedRouteHighlightsGroup: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: false },
  decorators: routeDecorator('/automation/policies'),
}

/** Collapsed sidebar: the active indicator still renders on the group icon. */
export const SidebarCollapsed: Story = {
  args: { onToggleSidebar: noOp, sidebarCollapsed: true },
  decorators: routeDecorator('/overview'),
}
