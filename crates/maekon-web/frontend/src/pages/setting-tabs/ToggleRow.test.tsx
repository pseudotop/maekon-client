/**
 * #13739: `ToggleRow` draws a `Switch`. Pins what the settings tabs rely on —
 * the row is one switch named by its text, a click anywhere on it changes the
 * value exactly once, and the #7678 `disabled` and #9465 `describedBy`
 * contracts survive the change from a checkbox.
 */
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import ToggleRow from './ToggleRow'

const LABEL = 'Process monitoring'
const DESCRIPTION = 'Track active applications and window titles'

describe('ToggleRow', () => {
  it('renders one switch named by the row text and reflecting the value', () => {
    render(<ToggleRow label={LABEL} description={DESCRIPTION} checked onChange={vi.fn()} />)

    const control = screen.getByRole('switch', { name: new RegExp(`^${LABEL}`) })
    expect(control).toBeChecked()
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument()
  })

  it.each([
    ['the switch', () => screen.getByRole('switch')],
    ['the label', () => screen.getByText(LABEL)],
    ['the description', () => screen.getByText(DESCRIPTION)],
  ])('reports the next value once when %s is clicked', async (_target, find) => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    render(<ToggleRow label={LABEL} description={DESCRIPTION} checked={false} onChange={onChange} />)

    await user.click(find())

    expect(onChange).toHaveBeenCalledTimes(1)
    expect(onChange).toHaveBeenCalledWith(true)
  })

  it('#7678 disables the switch without hiding the row', async () => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    render(<ToggleRow label={LABEL} description={DESCRIPTION} checked={false} onChange={onChange} disabled />)

    const control = screen.getByRole('switch', { name: new RegExp(`^${LABEL}`) })
    expect(control).toBeDisabled()
    expect(screen.getByText(DESCRIPTION)).toBeVisible()

    await user.click(control)
    await user.click(screen.getByText(LABEL))
    expect(onChange).not.toHaveBeenCalled()
  })

  it('#9465 points aria-describedby at exactly the supplied disclosure', () => {
    render(
      <>
        <ToggleRow
          label={LABEL}
          description={DESCRIPTION}
          checked={false}
          onChange={vi.fn()}
          describedBy="row-risk-disclosure"
        />
        <p id="row-risk-disclosure">Window titles can contain document names.</p>
      </>,
    )

    const control = screen.getByRole('switch', { name: new RegExp(`^${LABEL}`) })
    expect(control).toHaveAttribute('aria-describedby', 'row-risk-disclosure')
    expect(control).toHaveAccessibleDescription('Window titles can contain document names.')
  })

  it('adds no aria-describedby when no disclosure is supplied', () => {
    render(<ToggleRow label={LABEL} description={DESCRIPTION} checked={false} onChange={vi.fn()} />)

    expect(screen.getByRole('switch')).not.toHaveAttribute('aria-describedby')
  })
})
