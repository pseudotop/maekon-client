import type React from 'react'

export function highlightText(text: string, query: string): React.ReactNode {
  if (!query) return text
  const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const regex = new RegExp(`(${escaped})`, 'gi')
  const parts = text.split(regex)
  let offset = 0
  return parts.map((part) => {
    const key = `hl-${offset}`
    offset += part.length
    if (regex.test(part)) {
      return (
        <mark key={key} className="rounded bg-semantic-warning/25 px-0.5">
          {part}
        </mark>
      )
    }
    return <span key={key}>{part}</span>
  })
}
