/**
 * Full-shell integration tests that mirror the Storybook scenarios in
 * `ActivityBar.stories.tsx`, `SidePanel.stories.tsx`, and
 * `DesktopShellTemplate.stories.tsx`.
 *
 * These tests render the real `ActivityBar` + `SidePanel` pair together at
 * each of the documented routes and assert that the ActivityBar active
 * indicator and the SidePanel tree contents line up.  The goal is to catch
 * any drift between a story scenario and the actual component behaviour —
 * the storybook build only verifies compilation, not that the rendered DOM
 * matches the story's description.
 */

import { screen, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import ActivityBar from '../ActivityBar'
import SidePanel from '../SidePanel'

const noop = vi.fn

function FullShell({ sidebarCollapsed = false }: { sidebarCollapsed?: boolean }) {
  return (
    <div className="flex h-screen">
      <ActivityBar onToggleSidebar={noop()} sidebarCollapsed={sidebarCollapsed} />
      <SidePanel
        collapsed={sidebarCollapsed}
        width={260}
        onResizeStart={noop()}
        onResizeByKeyboard={noop()}
        onCollapse={noop()}
      />
    </div>
  )
}

interface ShellCase {
  route: string
  activeGroupTestId: string | null
  activeBottomTestId?: string
  expectedHeaderPattern: RegExp
  selectedLeafPattern: RegExp
  expectedTopLevelItems: RegExp[]
}

const CASES: ShellCase[] = [
  {
    route: '/overview',
    activeGroupTestId: 'nav-group-monitor',
    expectedHeaderPattern: /monitor/i,
    selectedLeafPattern: /overview/i,
    expectedTopLevelItems: [/dashboard/i, /day view/i, /timeline/i, /replay/i, /focus/i],
  },
  {
    // Note: `sidebar.policies` (automation child) translates to "Runtime
    // Status", not "Policies", because it's the first tab of the Automation
    // page.  The top-level `nav.policies` ("Execution Policies") is a
    // separate manage group route.
    route: '/automation/policies',
    activeGroupTestId: 'nav-group-manage',
    expectedHeaderPattern: /manage/i,
    selectedLeafPattern: /runtime status/i,
    expectedTopLevelItems: [/automation/i, /recalibration/i, /^execution policies$/i, /audit/i, /updates/i],
  },
  {
    route: '/day',
    activeGroupTestId: 'nav-group-monitor',
    expectedHeaderPattern: /monitor/i,
    selectedLeafPattern: /day view/i,
    expectedTopLevelItems: [/dashboard/i, /day view/i, /timeline/i, /replay/i, /focus/i],
  },
  {
    route: '/reports/activity',
    activeGroupTestId: 'nav-group-insights',
    expectedHeaderPattern: /insights/i,
    selectedLeafPattern: /activity report/i,
    expectedTopLevelItems: [/reports/i, /coaching/i, /chat/i, /playbooks/i, /search/i],
  },
  {
    route: '/chat',
    activeGroupTestId: 'nav-group-insights',
    expectedHeaderPattern: /insights/i,
    selectedLeafPattern: /chat/i,
    expectedTopLevelItems: [/reports/i, /coaching/i, /chat/i, /playbooks/i, /search/i],
  },
  {
    route: '/audit/summary',
    activeGroupTestId: 'nav-group-manage',
    expectedHeaderPattern: /manage/i,
    selectedLeafPattern: /summary/i,
    expectedTopLevelItems: [/automation/i, /recalibration/i, /^execution policies$/i, /audit/i, /updates/i],
  },
  {
    // childGroups: tabs grouped under "Core" and "Advanced" top-level treeitems
    route: '/settings/general',
    activeGroupTestId: null,
    activeBottomTestId: 'nav-settings',
    expectedHeaderPattern: /settings/i,
    selectedLeafPattern: /^general$/i,
    expectedTopLevelItems: [/^core$/i, /^advanced$/i],
  },
  {
    route: '/privacy/data',
    activeGroupTestId: null,
    activeBottomTestId: 'nav-privacy',
    expectedHeaderPattern: /privacy/i,
    selectedLeafPattern: /data controls/i,
    expectedTopLevelItems: [/data controls/i, /danger zone/i, /data export/i],
  },
]

function getTopLevelTreeItems(tree: HTMLElement): HTMLElement[] {
  return Array.from(tree.querySelectorAll<HTMLElement>('[role="treeitem"][aria-level="1"]'))
}

describe('Shell integration (ActivityBar + SidePanel story scenarios)', () => {
  for (const testCase of CASES) {
    it(`renders the right group + tree for ${testCase.route}`, () => {
      renderWithProviders(<FullShell />, {
        routerProps: { initialEntries: [testCase.route] },
      })

      // The correct ActivityBar icon shows aria-current="page".
      const active = screen.getByRole('button', { current: 'page' })
      const expectedTestId = testCase.activeGroupTestId ?? testCase.activeBottomTestId
      expect(active).toHaveAttribute('data-testid', expectedTestId)

      // SidePanel renders its tree.
      const tree = screen.getByRole('tree')
      expect(tree).toBeInTheDocument()

      // Every expected top-level item is visible (aria-level="1" only to
      // avoid matching duplicate labels like "Timeline" which exists both as
      // a top-level monitor route and as a child under /replay).
      const topLevelItems = getTopLevelTreeItems(tree)
      for (const pattern of testCase.expectedTopLevelItems) {
        const match = topLevelItems.find((el) => pattern.test(el.textContent ?? ''))
        expect(match, `expected a top-level treeitem matching ${pattern} at ${testCase.route}`).toBeDefined()
      }

      // The current leaf is selected somewhere in the tree (either top-level
      // for leaf routes like /day, or nested under the expanded parent for
      // routes like /automation/policies).
      const selected = within(tree).getByRole('treeitem', { selected: true })
      expect(selected).toHaveTextContent(testCase.selectedLeafPattern)
    })
  }

  it('suppresses the SidePanel tree when collapsed', () => {
    renderWithProviders(<FullShell sidebarCollapsed />, {
      routerProps: { initialEntries: ['/overview'] },
    })
    expect(screen.queryByRole('tree')).not.toBeInTheDocument()
    // ActivityBar still renders its buttons.
    expect(screen.getByTestId('nav-group-monitor')).toBeInTheDocument()
  })

  it('exposes the collapse affordance in the SidePanel header when active', () => {
    renderWithProviders(<FullShell />, {
      routerProps: { initialEntries: ['/overview'] },
    })
    expect(screen.getByTestId('sidepanel-collapse')).toBeInTheDocument()
  })
})
