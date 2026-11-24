import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import TreeView, { type TreeNode } from '../TreeView'

const sampleNodes: TreeNode[] = [
  { id: 'a', label: 'Alpha' },
  {
    id: 'b',
    label: 'Beta',
    children: [
      { id: 'b1', label: 'Beta-1' },
      { id: 'b2', label: 'Beta-2' },
    ],
  },
  { id: 'c', label: 'Charlie' },
]

describe('TreeView', () => {
  it('has displayName', () => {
    expect(TreeView.displayName).toBe('TreeView')
  })

  it('renders treeitems', () => {
    renderWithProviders(<TreeView nodes={sampleNodes} />)
    const items = screen.getAllByRole('treeitem')
    // Alpha + Beta (expanded by default) + Beta-1 + Beta-2 + Charlie = 5
    expect(items.length).toBe(5)
  })

  it('renders tree role at root', () => {
    renderWithProviders(<TreeView nodes={sampleNodes} />)
    expect(screen.getByRole('tree')).toBeInTheDocument()
  })

  it('collapse on click toggles children visibility', async () => {
    const user = userEvent.setup()
    renderWithProviders(<TreeView nodes={sampleNodes} />)

    // Beta is expanded by default — click to collapse
    const betaItem = screen.getByText('Beta')
    await user.click(betaItem)

    // After collapsing, Beta-1 and Beta-2 should be gone
    expect(screen.queryByText('Beta-1')).not.toBeInTheDocument()
  })

  it('fires onSelect callback', async () => {
    const user = userEvent.setup()
    const onSelect = vi.fn()
    renderWithProviders(<TreeView nodes={sampleNodes} onSelect={onSelect} />)

    await user.click(screen.getByText('Alpha'))
    expect(onSelect).toHaveBeenCalledWith('a')
  })

  it('marks selected item with aria-selected', () => {
    renderWithProviders(<TreeView nodes={sampleNodes} selectedId="a" />)
    const alphaItem = screen.getByText('Alpha').closest('[role="treeitem"]')
    expect(alphaItem).toHaveAttribute('aria-selected', 'true')
  })
})

// #13736: the main side-panel tree folds its bundles and opens only the path
// to the current page.
const bundledNodes: TreeNode[] = [
  { id: '/front', label: 'Front' },
  {
    id: '__section-more',
    label: 'More',
    children: [
      { id: '/day', label: 'Day' },
      {
        id: '/replay',
        label: 'Replay',
        children: [
          { id: '/replay/timeline', label: 'Playback' },
          { id: '/replay/events', label: 'Events' },
        ],
      },
    ],
  },
  {
    id: '__section-tools',
    label: 'Tools',
    children: [{ id: '/automation', label: 'Automation' }],
  },
]

function treeItem(label: string) {
  return screen.getByText(label).closest('[role="treeitem"]') as HTMLElement
}

describe('TreeView expandMode="selected"', () => {
  it('opens only the ancestors of the selected node', () => {
    renderWithProviders(<TreeView nodes={bundledNodes} selectedId="/replay/events" expandMode="selected" />)
    expect(treeItem('More')).toHaveAttribute('aria-expanded', 'true')
    expect(treeItem('Replay')).toHaveAttribute('aria-expanded', 'true')
    expect(treeItem('Events')).toHaveAttribute('aria-selected', 'true')
    expect(treeItem('Tools')).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByText('Automation')).not.toBeInTheDocument()
  })

  it('starts fully folded when nothing is selected', () => {
    renderWithProviders(<TreeView nodes={bundledNodes} expandMode="selected" />)
    expect(treeItem('More')).toHaveAttribute('aria-expanded', 'false')
    expect(treeItem('Tools')).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByText('Day')).not.toBeInTheDocument()
  })

  it('reveals a selection that moves into a folded bundle', () => {
    const { rerender } = renderWithProviders(
      <TreeView nodes={bundledNodes} selectedId="/front" expandMode="selected" />,
    )
    expect(screen.queryByText('Automation')).not.toBeInTheDocument()

    rerender(<TreeView nodes={bundledNodes} selectedId="/automation" expandMode="selected" />)
    expect(treeItem('Tools')).toHaveAttribute('aria-expanded', 'true')
    expect(treeItem('Automation')).toHaveAttribute('aria-selected', 'true')
  })

  it('keeps a bundle the user folded closed until the selection moves again', async () => {
    const user = userEvent.setup()
    renderWithProviders(<TreeView nodes={bundledNodes} selectedId="/day" expandMode="selected" />)
    await user.click(treeItem('More'))
    expect(treeItem('More')).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByText('Day')).not.toBeInTheDocument()
  })

  it('expands and collapses a nested parent from the keyboard', async () => {
    const user = userEvent.setup()
    renderWithProviders(<TreeView nodes={bundledNodes} selectedId="/day" expandMode="selected" />)
    treeItem('Replay').focus()

    await user.keyboard('{ArrowRight}')
    expect(treeItem('Replay')).toHaveAttribute('aria-expanded', 'true')
    expect(screen.getByText('Playback')).toBeInTheDocument()

    await user.keyboard('{ArrowLeft}')
    expect(treeItem('Replay')).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByText('Playback')).not.toBeInTheDocument()
  })
})

describe('TreeView default expandMode', () => {
  it('opens every parent at every depth', () => {
    renderWithProviders(<TreeView nodes={bundledNodes} />)
    expect(treeItem('More')).toHaveAttribute('aria-expanded', 'true')
    expect(treeItem('Replay')).toHaveAttribute('aria-expanded', 'true')
    expect(treeItem('Tools')).toHaveAttribute('aria-expanded', 'true')
    expect(screen.getByText('Playback')).toBeInTheDocument()
  })
})
