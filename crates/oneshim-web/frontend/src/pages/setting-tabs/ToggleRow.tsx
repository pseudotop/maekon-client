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
}

export default function ToggleRow({ label, description, checked, onChange }: ToggleRowProps) {
  return (
    <label className="flex cursor-pointer items-center justify-between">
      <div>
        <span className={colors.text.secondary}>{label}</span>
        <p className={colors.text.tertiary}>{description}</p>
      </div>
      <Checkbox checked={checked} onChange={(e) => onChange(e.target.checked)} />
    </label>
  )
}
