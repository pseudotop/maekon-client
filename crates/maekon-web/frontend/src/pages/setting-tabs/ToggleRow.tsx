/**
 *
 */
import { Checkbox } from '../../components/ui'
import { colors } from '../../styles/tokens'

export interface ToggleRowProps {
  label: string
  description: string
  checked: boolean
  onChange: (checked: boolean) => void
  /** #7678: disables the underlying checkbox (e.g. a platform-capability gap) without hiding the row. */
  disabled?: boolean
}

export default function ToggleRow({ label, description, checked, onChange, disabled = false }: ToggleRowProps) {
  return (
    <label
      className={`flex items-center justify-between ${disabled ? 'cursor-not-allowed opacity-50' : 'cursor-pointer'}`}
    >
      <div>
        <span className={colors.text.secondary}>{label}</span>
        <p className={colors.text.tertiary}>{description}</p>
      </div>
      <Checkbox checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
    </label>
  )
}
