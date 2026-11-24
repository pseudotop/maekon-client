import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { Switch } from '../Switch'

function ControlledSwitch({ initial = false, disabled = false }: { initial?: boolean; disabled?: boolean }) {
  const [checked, setChecked] = useState(initial)
  return <Switch label="Process monitoring" checked={checked} onCheckedChange={setChecked} disabled={disabled} />
}

describe('Switch', () => {
  it('exposes role="switch" with aria-checked that follows the value', async () => {
    const user = userEvent.setup()
    render(<ControlledSwitch />)

    const control = screen.getByRole('switch', { name: 'Process monitoring' })
    expect(control).toHaveAttribute('aria-checked', 'false')
    expect(control).not.toBeChecked()

    await user.click(control)

    expect(control).toHaveAttribute('aria-checked', 'true')
    expect(control).toBeChecked()
  })

  it('requests the opposite value without changing it on its own', async () => {
    const user = userEvent.setup()
    const onCheckedChange = vi.fn()
    render(<Switch aria-label="Process monitoring" checked onCheckedChange={onCheckedChange} />)

    const control = screen.getByRole('switch', { name: 'Process monitoring' })
    await user.click(control)

    expect(onCheckedChange).toHaveBeenCalledTimes(1)
    expect(onCheckedChange).toHaveBeenCalledWith(false)
    // Controlled: the caller has not accepted the change yet.
    expect(control).toHaveAttribute('aria-checked', 'true')
  })

  it('toggles from the keyboard with Space and Enter', async () => {
    const user = userEvent.setup()
    render(<ControlledSwitch />)

    const control = screen.getByRole('switch', { name: 'Process monitoring' })
    await user.tab()
    expect(control).toHaveFocus()

    await user.keyboard(' ')
    expect(control).toHaveAttribute('aria-checked', 'true')

    await user.keyboard('{Enter}')
    expect(control).toHaveAttribute('aria-checked', 'false')
  })

  it('carries the shared keyboard focus ring', () => {
    render(<Switch aria-label="Process monitoring" checked={false} />)

    // jsdom does not paint :focus-visible, so pin the token classes that draw it.
    const control = screen.getByRole('switch', { name: 'Process monitoring' })
    expect(control).toHaveClass(
      'focus-visible:ring-2',
      'focus-visible:ring-brand-signal',
      'focus-visible:ring-offset-2',
    )
  })

  it('cannot be focused or toggled while disabled', async () => {
    const user = userEvent.setup()
    const onCheckedChange = vi.fn()
    render(
      <>
        <Switch label="Process monitoring" checked={false} onCheckedChange={onCheckedChange} disabled />
        <button type="button">After</button>
      </>,
    )

    const control = screen.getByRole('switch', { name: 'Process monitoring' })
    expect(control).toBeDisabled()
    expect(control).toHaveClass('disabled:opacity-50', 'disabled:cursor-not-allowed')

    await user.tab()
    expect(control).not.toHaveFocus()
    expect(screen.getByRole('button', { name: 'After' })).toHaveFocus()

    await user.click(control)
    await user.click(screen.getByText('Process monitoring'))
    expect(onCheckedChange).not.toHaveBeenCalled()
    expect(control).toHaveAttribute('aria-checked', 'false')
  })

  it('links its description and caller-supplied ids through aria-describedby', () => {
    render(
      <>
        <Switch
          label="Mirror to a folder"
          description="Writes a copy of your memory graph."
          aria-describedby="vault-disclosure"
          checked={false}
        />
        <p id="vault-disclosure">One-way: edits in the folder are never read back.</p>
      </>,
    )

    const control = screen.getByRole('switch', { name: 'Mirror to a folder' })
    expect(control).toHaveAccessibleDescription(
      'Writes a copy of your memory graph. One-way: edits in the folder are never read back.',
    )
  })

  it('passes aria-describedby through unchanged when it has no description', () => {
    render(<Switch aria-label="Process monitoring" aria-describedby="risk-note" checked={false} />)

    expect(screen.getByRole('switch', { name: 'Process monitoring' })).toHaveAttribute('aria-describedby', 'risk-note')
  })

  it('toggles when its visible label is clicked', async () => {
    const user = userEvent.setup()
    render(<ControlledSwitch />)

    await user.click(screen.getByText('Process monitoring'))

    expect(screen.getByRole('switch', { name: 'Process monitoring' })).toHaveAttribute('aria-checked', 'true')
  })

  it('never submits the form it sits in', async () => {
    const user = userEvent.setup()
    const onSubmit = vi.fn((event: React.FormEvent) => event.preventDefault())
    render(
      <form onSubmit={onSubmit}>
        <ControlledSwitch />
      </form>,
    )

    const control = screen.getByRole('switch', { name: 'Process monitoring' })
    await user.click(control)
    control.focus()
    await user.keyboard('{Enter}')

    expect(control).toHaveAttribute('type', 'button')
    expect(onSubmit).not.toHaveBeenCalled()
  })
})
