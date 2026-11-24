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
  /** Labels that must NOT sit at the first tree level (demoted one level back). */
  absentTopLevelItems?: RegExp[]
}

// #13736: the main tree shows six front destinations, then the More / Tools /
// System bundles that hold everything else one level back.
const MAIN_TOP_LEVEL = [
  /^dashboard$/i,
  /^timeline$/i,
  /^search$/i,
  /^focus$/i,
  /^reports$/i,
  /^chat$/i,
  /^more$/i,
  /^tools$/i,
  /^system$/i,
]
const DEMOTED = [/day view/i, /replay/i, /automation/i, /execution policies/i, /audit log/i, /updates/i, /support/i]

const CASES: ShellCase[] = [
  {
    route: '/overview',
    activeGroupTestId: 'nav-group-main',
    expectedHeaderPattern: /menu/i,
    selectedLeafPattern: /overview/i,
    expectedTopLevelItems: MAIN_TOP_LEVEL,
    absentTopLevelItems: DEMOTED,
  },
  {
    // Note: `sidebar.policies` (automation child) translates to "Runtime
    // Status", not "Policies", because it's the first tab of the Automation
    // page.  The top-level `nav.policies` ("Execution Policies") is a
    // separate route in the same Tools bundle.  The selected leaf sits two
    // levels down (Tools › Automation › Runtime Status) and is revealed.
    route: '/automation/policies',
    activeGroupTestId: 'nav-group-main',
    expectedHeaderPattern: /menu/i,
    selectedLeafPattern: /runtime status/i,
    expectedTopLevelItems: MAIN_TOP_LEVEL,
    absentTopLevelItems: DEMOTED,
  },
  {
    route: '/day',
    activeGroupTestId: 'nav-group-main',
    expectedHeaderPattern: /menu/i,
    selectedLeafPattern: /day view/i,
    expectedTopLevelItems: MAIN_TOP_LEVEL,
    absentTopLevelItems: DEMOTED,
  },
  {
    route: '/reports/activity',
    activeGroupTestId: 'nav-group-main',
    expectedHeaderPattern: /menu/i,
    selectedLeafPattern: /activity report/i,
    expectedTopLevelItems: MAIN_TOP_LEVEL,
  },
  {
    route: '/chat',
    activeGroupTestId: 'nav-group-main',
    expectedHeaderPattern: /menu/i,
    selectedLeafPattern: /chat/i,
    expectedTopLevelItems: MAIN_TOP_LEVEL,
  },
  {
    route: '/audit/summary',
    activeGroupTestId: 'nav-group-main',
    expectedHeaderPattern: /menu/i,
    selectedLeafPattern: /summary/i,
    expectedTopLevelItems: MAIN_TOP_LEVEL,
    absentTopLevelItems: DEMOTED,
  },
  {
    // Support left the rail for the System bundle; the rail's main group
    // icon is the active one there.
    route: '/support',
    activeGroupTestId: 'nav-group-main',
    expectedHeaderPattern: /menu/i,
    selectedLeafPattern: /support/i,
    expectedTopLevelItems: MAIN_TOP_LEVEL,
    absentTopLevelItems: DEMOTED,
  },
  {
    // childGroups: tabs grouped under "Core" and "More Settings" top-level
    // treeitems — the "Advanced" tab no longer sits inside an "Advanced" group.
    route: '/settings/general',
    activeGroupTestId: null,
    activeBottomTestId: 'nav-settings',
    expectedHeaderPattern: /settings/i,
    selectedLeafPattern: /^general$/i,
    expectedTopLevelItems: [/^core$/i, /^more settings$/i],
    absentTopLevelItems: [/^advanced$/i],
  },
  {
    route: '/privacy/data',
    activeGroupTestId: null,
    activeBottomTestId: 'nav-privacy',
    expectedHeaderPattern: /privacy/i,
    selectedLeafPattern: /data controls/i,
    expectedTopLevelItems: [/data controls/i, /danger zone/i, /backup & export/i],
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

      // Every expected top-level item is visible (aria-level="1" only, so a
      // nested label can never satisfy a first-level expectation).
      const topLevelItems = getTopLevelTreeItems(tree)
      for (const pattern of testCase.expectedTopLevelItems) {
        const match = topLevelItems.find((el) => pattern.test(el.textContent ?? ''))
        expect(match, `expected a top-level treeitem matching ${pattern} at ${testCase.route}`).toBeDefined()
      }
      for (const pattern of testCase.absentTopLevelItems ?? []) {
        const match = topLevelItems.find((el) => pattern.test(el.textContent ?? ''))
        expect(match, `expected no top-level treeitem matching ${pattern} at ${testCase.route}`).toBeUndefined()
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
    expect(screen.getByTestId('nav-group-main')).toBeInTheDocument()
  })

  it('exposes the collapse affordance in the SidePanel header when active', () => {
    renderWithProviders(<FullShell />, {
      routerProps: { initialEntries: ['/overview'] },
    })
    expect(screen.getByTestId('sidepanel-collapse')).toBeInTheDocument()
  })
})
