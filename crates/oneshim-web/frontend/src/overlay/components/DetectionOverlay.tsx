import { forwardRef, useCallback, useEffect, useRef } from 'react'
import { palette } from '../../styles/tokens'
import type { DetectionElementPayload, DetectionScenePayload } from '../types'

const ROLE_COLORS: Record<string, string> = {
  AXButton: palette.blue500,
  button: palette.blue500,
  Button: palette.blue500,
  AXTextField: palette.green500,
  AXTextArea: palette.green500,
  edit: palette.green500,
  Edit: palette.green500,
  TextInput: palette.green500,
  AXLink: palette.violet500,
  link: palette.violet500,
  Link: palette.violet500,
  AXMenuItem: palette.orange500,
  menuitem: palette.orange500,
  MenuItem: palette.orange500,
  AXTabGroup: palette.cyan500,
  tab: palette.cyan500,
  TabLabel: palette.cyan500,
  AXOutlineRow: palette.amber500,
  treeitem: palette.amber500,
  TreeItem: palette.amber500,
  AXImage: palette.pink500,
  image: palette.pink500,
  Image: palette.pink500,
}

const DEFAULT_COLOR = palette.gray500

function getRoleColor(role: string | null): string {
  if (!role) return DEFAULT_COLOR
  return ROLE_COLORS[role] ?? DEFAULT_COLOR
}

function getRoleLabel(role: string | null): string {
  if (!role) return '?'
  return role.replace(/^AX/, '')
}

interface DetectionOverlayProps {
  scene: DetectionScenePayload
  selectedId: string | null
  onSelect: (id: string | null) => void
}

export default function DetectionOverlay({ scene, selectedId, onSelect }: DetectionOverlayProps) {
  const selected = selectedId ? scene.elements.find((el) => el.element_id === selectedId) : null

  const handleBoxClick = useCallback(
    (el: DetectionElementPayload) => {
      onSelect(selectedId === el.element_id ? null : el.element_id)
    },
    [selectedId, onSelect],
  )

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        if (selectedId) {
          onSelect(null)
        } else {
          import('@tauri-apps/api/core')
            .then(({ invoke }) => {
              invoke('toggle_detection_overlay', { active: false })
            })
            .catch((e) => console.warn('toggle_detection_overlay failed:', e))
        }
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [selectedId, onSelect])

  return (
    <>
      {scene.elements.map((el) => {
        const color = getRoleColor(el.role)
        const opacity = el.confidence * 0.6 + 0.4
        const isSelected = el.element_id === selectedId

        return (
          <button
            type="button"
            key={el.element_id}
            aria-label={`${getRoleLabel(el.role)}: ${el.label || 'unlabeled'}`}
            className="fixed cursor-pointer"
            style={{
              left: el.x,
              top: el.y,
              width: el.width,
              height: el.height,
              outline: `${isSelected ? 2 : 1.5}px solid ${color}`,
              outlineOffset: '-1px',
              borderRadius: '2px',
              backgroundColor: `${color}${isSelected ? '20' : '14'}`,
              opacity,
              zIndex: isSelected ? 10001 : 10000,
              transition: 'outline-width 0.1s, background-color 0.1s',
            }}
            onClick={(e) => {
              e.stopPropagation()
              handleBoxClick(el)
            }}
          >
            <span
              className="absolute top-0.5 left-0.5 rounded px-0.5 font-medium text-[8px] leading-3"
              style={{
                backgroundColor: `${color}CC`,
                color: 'rgb(var(--content-inverse))',
                whiteSpace: 'nowrap',
              }}
            >
              {getRoleLabel(el.role)}
            </span>
          </button>
        )
      })}

      {selected && <Inspector element={selected} />}
    </>
  )
}

interface InspectorProps {
  element: DetectionElementPayload
}

const Inspector = forwardRef<HTMLDivElement, InspectorProps>(function Inspector({ element }, ref) {
  const color = getRoleColor(element.role)
  const innerRef = useRef<HTMLDivElement>(null)
  const resolvedRef = (ref as React.RefObject<HTMLDivElement>) ?? innerRef

  const left = Math.min(element.x + element.width + 8, window.innerWidth - 300)
  const top = Math.min(element.y, window.innerHeight - 200)

  return (
    <div
      ref={resolvedRef}
      className="fixed z-detection-inspector rounded-lg border border-white/20 bg-black/85 p-3 text-white text-xs shadow-2xl backdrop-blur-sm"
      style={{ left, top, width: 260 }}
    >
      <div className="mb-1.5 flex items-center justify-between">
        <span className="rounded px-1.5 py-0.5 font-semibold text-[10px]" style={{ backgroundColor: `${color}CC` }}>
          {getRoleLabel(element.role)}
        </span>
        <span className="text-white/60">{(element.confidence * 100).toFixed(0)}%</span>
      </div>
      <div className="space-y-1 text-[11px]">
        <Row label="label" value={element.label || '(empty)'} />
        <Row label="role" value={element.role ?? 'unknown'} />
        <Row label="bounds" value={`(${element.x}, ${element.y}, ${element.width}, ${element.height})`} />
        <Row label="source" value={element.source} />
        <Row label="id" value={element.element_id} mono />
      </div>
    </div>
  )
})

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex justify-between gap-2">
      <span className="text-white/50">{label}</span>
      <span className={`text-right ${mono ? 'font-mono' : ''} truncate`} title={value}>
        {value}
      </span>
    </div>
  )
}
