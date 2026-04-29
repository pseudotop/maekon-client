/**
 *
 */
import { forwardRef, useId } from 'react'
import { form, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

export interface CheckboxProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'type'> {
  label?: string
  description?: string
}

export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(
  ({ className, label, description, id: externalId, ...props }, ref) => {
    const autoId = useId()
    const id = externalId ?? autoId

    if (!label) {
      return <input ref={ref} id={id} type="checkbox" className={cn(form.checkbox, className)} {...props} />
    }

    return (
      <div className="flex items-start gap-3">
        <input ref={ref} id={id} type="checkbox" className={cn(form.checkbox, 'mt-0.5', className)} {...props} />
        <div>
          <label htmlFor={id} className={cn(typography.label, 'cursor-pointer text-content')}>
            {label}
          </label>
          {description && <p className={cn(typography.caption, 'mt-0.5 text-content-secondary')}>{description}</p>}
        </div>
      </div>
    )
  },
)

Checkbox.displayName = 'Checkbox'
