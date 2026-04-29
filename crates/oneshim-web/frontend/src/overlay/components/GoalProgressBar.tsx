import { memo } from 'react'
import { motion } from '../../styles/tokens'
import type { GoalProgressItem } from '../types'

interface GoalProgressBarProps {
  goals: GoalProgressItem[]
}

export default memo(function GoalProgressBar({ goals }: GoalProgressBarProps) {
  return (
    <div className="fixed inset-x-0 bottom-0 z-dropdown">
      {/* No per-element cursor passthrough — overlay interactivity is toggled
          globally via Cmd+Shift+O hotkey */}
      <div className="mx-auto max-w-3xl rounded-t-lg border border-content-inverse/10 border-b-0 bg-surface-sunken/80 px-4 py-2 backdrop-blur-md">
        <div className="flex flex-wrap gap-3">
          {goals.map((goal) => (
            <div key={goal.regime_label} className="flex min-w-[180px] flex-1 items-center gap-2">
              <span className="w-20 truncate text-content-secondary text-xs">{goal.regime_label}</span>
              <div
                className="relative h-2 flex-1 overflow-hidden rounded-full bg-content-inverse/10"
                role="progressbar"
                aria-label={`${goal.regime_label} progress`}
                aria-valuemin={0}
                aria-valuenow={goal.current_minutes}
                aria-valuemax={goal.target_minutes}
              >
                <div
                  className={`absolute inset-y-0 left-0 rounded-full ${motion.all}`}
                  style={{
                    width: `${Math.min(goal.percentage ?? 0, 100)}%`,
                    backgroundColor: goal.display_color,
                  }}
                />
              </div>
              <span className="w-16 text-right text-content-tertiary text-xs">
                {goal.current_minutes ?? 0}/{goal.target_minutes ?? 0}m
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
})
