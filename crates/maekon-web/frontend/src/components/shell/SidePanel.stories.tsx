import type { Meta, StoryObj } from '@storybook/react'
import { AppMemoryRouter } from '../../router/future'
import SidePanel from './SidePanel'

const meta = {
  title: 'Shell/SidePanel',
  component: SidePanel,
  tags: ['autodocs'],
  parameters: {
    docs: {
      description: {
        component: `
The SidePanel has two distinct render modes after the category restructure:

1. **Group mode** (default for any route that belongs to a nav group). Shows
   the full group tree — every top-level route in the group is a first-level
   treeitem, and each route's \`children\` nest underneath.  The current
   pathname highlights the most-specific match.

2. **Bottom-route mode** (Settings, Privacy). Falls back to the legacy
   "children of the current route" view.  Bottom routes are outside the group
   hierarchy so their sub-tabs render directly.

Both modes render a **collapse button** in the header (PanelLeftClose icon)
so users can hide the panel without the \`Cmd/Ctrl+B\` shortcut.
`.trim(),
      },
    },
  },
  decorators: [
    (Story) => (
      <AppMemoryRouter initialEntries={['/overview']}>
        <div style={{ height: 500 }} className="flex">
          <Story />
        </div>
      </AppMemoryRouter>
    ),
  ],
} satisfies Meta<typeof SidePanel>

export default meta
type Story = StoryObj<typeof meta>

const routeDecorator = (route: string): Meta<typeof SidePanel>['decorators'] => [
  (Story) => (
    <AppMemoryRouter initialEntries={[route]}>
      <div style={{ height: 500 }} className="flex">
        <Story />
      </div>
    </AppMemoryRouter>
  ),
]

const noOp = () => {}

const baseArgs = {
  collapsed: false,
  width: 260,
  onResizeStart: noOp,
  onResizeByKeyboard: noOp,
  onCollapse: noOp,
}

/** Monitor group tree rendered from /overview (Dashboard.Overview selected). */
export const MonitorGroupTree: Story = {
  args: baseArgs,
}

/** Data group tree rendered from /reports/activity. */
export const DataGroupTree: Story = {
  args: baseArgs,
  decorators: routeDecorator('/reports/activity'),
}

/** Manage group tree rendered from /audit/summary. */
export const ManageGroupTree: Story = {
  args: baseArgs,
  decorators: routeDecorator('/audit/summary'),
}

/** Nested child highlighted — /automation/policies → Policies leaf selected. */
export const NestedChildHighlighted: Story = {
  args: baseArgs,
  decorators: routeDecorator('/automation/policies'),
}

/** Leaf-only route — /day highlights the Day View leaf in the Monitor tree. */
export const LeafRouteInGroup: Story = {
  args: baseArgs,
  decorators: routeDecorator('/day'),
}

/** Chat (leaf in Data group) — tree shows every Data route, Chat selected. */
export const LeafRouteChatInData: Story = {
  args: baseArgs,
  decorators: routeDecorator('/chat'),
}

/** Settings bottom route — legacy mode, shows only Settings children. */
export const SettingsLegacyMode: Story = {
  args: baseArgs,
  decorators: routeDecorator('/settings/general'),
}

/** Privacy bottom route — legacy mode, shows only Privacy children. */
export const PrivacyLegacyMode: Story = {
  args: baseArgs,
  decorators: routeDecorator('/privacy/data'),
}

/** Collapsed — the panel returns null and nothing renders. */
export const Collapsed: Story = {
  args: { ...baseArgs, collapsed: true },
}

/** Without a collapse handler — the chevron button is suppressed. */
export const WithoutCollapseButton: Story = {
  args: { ...baseArgs, onCollapse: undefined },
}
