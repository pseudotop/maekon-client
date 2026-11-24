/**
 * #13739: the shared control for a setting that is either on or off.
 *
 * It is the WAI-ARIA APG button switch: `<button type="button" role="switch"
 * aria-checked>`. A native button already activates on Space and Enter, takes
 * keyboard focus and honours `disabled`, and the fixed `type="button"` keeps it
 * from submitting the settings `<form>` it usually sits in.
 *
 * Keep `Checkbox` for picking several items from a list and for confirmation
 * checks ("I understand ..."); those are not on/off settings.
 *
 * Name the switch with `label`, or from outside with a wrapping `<label>`,
 * `aria-label` or `aria-labelledby`. `description` is linked through
 * `aria-describedby`, ahead of any ids the caller passes there.
 */
import { forwardRef, useId } from 'react'
import { form, interaction, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'

export interface SwitchProps
  extends Omit<
    React.ButtonHTMLAttributes<HTMLButtonElement>,
    'type' | 'role' | 'value' | 'onChange' | 'children' | 'aria-checked'
  > {
  /** Whether the setting is on. The switch is controlled: the caller owns the value. */
  checked: boolean
  /** Receives the requested next value when the user toggles the switch. */
  onCheckedChange?: (checked: boolean) => void
  /** Visible label, associated through `<label htmlFor>`. */
  label?: string
  /** Supplementary text under `label`, associated through `aria-describedby`. */
  description?: string
}

export const Switch = forwardRef<HTMLButtonElement, SwitchProps>(
  (
    {
      checked,
      onCheckedChange,
      onClick,
      label,
      description,
      id: externalId,
      className,
      'aria-describedby': ariaDescribedBy,
      ...props
    },
    ref,
  ) => {
    const autoId = useId()
    const id = externalId ?? autoId
    const descriptionId = label && description ? `${id}-description` : undefined
    const describedBy = [descriptionId, ariaDescribedBy].filter(Boolean).join(' ') || undefined

    const control = (
      <button
        ref={ref}
        id={id}
        type="button"
        role="switch"
        aria-checked={checked}
        aria-describedby={describedBy}
        className={cn(
          form.switchTrack,
          checked ? form.switchTrackOn : form.switchTrackOff,
          interaction.focusRing,
          form.switchFocusOffset,
          interaction.disabled,
          className,
        )}
        onClick={(event) => {
          onClick?.(event)
          if (!event.defaultPrevented) onCheckedChange?.(!checked)
        }}
        {...props}
      >
        <span aria-hidden="true" className={cn(form.switchThumb, checked ? form.switchThumbOn : form.switchThumbOff)} />
      </button>
    )

    if (!label) return control

    return (
      <div className="flex items-start gap-3">
        {control}
        <div className="pt-0.5">
          <label htmlFor={id} className={cn(typography.label, 'cursor-pointer text-content')}>
            {label}
          </label>
          {description && (
            <p id={descriptionId} className={cn(typography.caption, 'mt-0.5 text-content-secondary')}>
              {description}
            </p>
          )}
        </div>
      </div>
    )
  },
)

Switch.displayName = 'Switch'
