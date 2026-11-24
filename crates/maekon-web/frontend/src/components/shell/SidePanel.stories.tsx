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
The SidePanel has two distinct render modes:

1. **Group mode** (default for any route in the main group). Shows the main
   tree — the six front destinations as first-level treeitems, then the
   More / Tools / System bundles that hold every other route one level back
   (#13736). Bundles start folded except the one holding the current page;
   each route's \`children\` nest underneath, and the current pathname
   highlights the most-specific match.

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

/** Main tree rendered from /overview (Dashboard.Overview selected, bundles folded). */
export const MainTree: Story = {
  args: baseArgs,
}

/** Front destination with children — /reports/activity opens Reports only. */
export const FrontDestinationTree: Story = {
  args: baseArgs,
  decorators: routeDecorator('/reports/activity'),
}

/** Bundled route — /audit/summary opens the Tools bundle and the Audit Log parent. */
export const BundledRouteTree: Story = {
  args: baseArgs,
  decorators: routeDecorator('/audit/summary'),
}

/** Nested child highlighted — /automation/policies → Runtime Status leaf selected. */
export const NestedChildHighlighted: Story = {
  args: baseArgs,
  decorators: routeDecorator('/automation/policies'),
}

/** Leaf-only route — /day highlights the Day View leaf inside the More bundle. */
export const LeafRouteInGroup: Story = {
  args: baseArgs,
  decorators: routeDecorator('/day'),
}

/** Chat (front leaf) — main tree with Chat selected and every bundle folded. */
export const FrontLeafRouteChat: Story = {
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
