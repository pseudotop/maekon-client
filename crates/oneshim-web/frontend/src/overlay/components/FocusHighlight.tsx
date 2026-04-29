import { memo } from 'react'
import { motion } from '../../styles/tokens'
import type { FocusHighlightPayload } from '../types'

interface FocusHighlightProps {
  highlight: FocusHighlightPayload
}

export default memo(function FocusHighlight({ highlight }: FocusHighlightProps) {
  return (
    <>
      {highlight.targets.map((target) => (
        <div
          key={target.candidate_id}
          className={`pointer-events-none fixed ${motion.all}`}
          style={{
            left: target.x,
            top: target.y,
            width: target.width,
            height: target.height,
            border: `2px solid ${target.color}`,
            borderRadius: '4px',
            opacity: 0.8,
            boxShadow: `0 0 12px ${target.color}40`,
          }}
        >
          {target.label && (
            <span
              className="absolute -top-5 left-0 rounded bg-surface-overlay/70 px-1 text-[10px] text-content-inverse"
              style={{ whiteSpace: 'nowrap' }}
            >
              {target.label}
            </span>
          )}
        </div>
      ))}
    </>
  )
})
