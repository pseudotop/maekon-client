/**
 * #13739: one on/off setting — label and description on the left, `Switch` on
 * the right. The whole row is the switch's `<label>`, so clicking anywhere on it
 * toggles the setting and the row text stays the switch's accessible name, as it
 * was when the row drew a checkbox.
 */
import { Switch } from '../../components/ui'
import { colors } from '../../styles/tokens'
import { cn } from '../../utils/cn'

export interface ToggleRowProps {
  label: string
  description: string
  checked: boolean
  onChange: (checked: boolean) => void
  /** #7678: disables the underlying switch (e.g. a platform-capability gap) without hiding the row. */
  disabled?: boolean
  /**
   * #9465: id of a supplementary disclosure to associate with the switch via
   * `aria-describedby`. A static `role="alert"` sibling is not reliably
   * announced (WAI-ARIA), so a toggle whose risk disclosure lives outside this
   * row must point at it explicitly — the same treatment
   * `ConsentToggleSection`'s `EnumeratedToggle` gives its `describedById`.
   */
  describedBy?: string
}

export default function ToggleRow({
  label,
  description,
  checked,
  onChange,
  disabled = false,
  describedBy,
}: ToggleRowProps) {
  return (
    <label
      className={cn('flex items-center justify-between gap-4', disabled ? 'cursor-not-allowed' : 'cursor-pointer')}
    >
      <span className={cn('min-w-0', disabled && 'opacity-50')}>
        <span className={cn('block', colors.text.secondary)}>{label}</span>
        <span className={cn('block', colors.text.tertiary)}>{description}</span>
      </span>
      <Switch checked={checked} disabled={disabled} aria-describedby={describedBy} onCheckedChange={onChange} />
    </label>
  )
}
