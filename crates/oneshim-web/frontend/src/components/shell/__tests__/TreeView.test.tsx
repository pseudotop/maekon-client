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
