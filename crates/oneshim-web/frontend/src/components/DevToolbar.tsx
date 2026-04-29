/**
 * Dev-only debug toolbar. Only rendered when import.meta.env.DEV is true.
 * Provides quick toggles for common dev tasks without needing Console access.
 *
 * NOTE: This component intentionally uses raw Tailwind colors (not design tokens).
 * It is dev-only and not part of the shipped UI.
 */
import { useState } from 'react'
import { Divider } from './ui'

const STANDALONE_KEY = 'oneshim-web-standalone-mode'

function isStandalone(): boolean {
  return localStorage.getItem(STANDALONE_KEY) === '1'
}

export function DevToolbar() {
  const [open, setOpen] = useState(false)
  const [standalone, setStandalone] = useState(isStandalone)

  if (!import.meta.env.DEV) return null

  const toggleStandalone = () => {
    const next = !standalone
    localStorage.setItem(STANDALONE_KEY, next ? '1' : '0')
    setStandalone(next)
    location.reload()
  }

  const clearStorage = () => {
    localStorage.clear()
    location.reload()
  }

  const apiBase = window.__ONESHIM_WEB_PORT__
    ? `http://127.0.0.1:${window.__ONESHIM_WEB_PORT__}`
    : 'http://127.0.0.1:10090'

  if (!open) {
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="fixed right-4 bottom-12 z-[9999] rounded-full bg-yellow-500 px-2 py-1 font-bold text-black text-xs opacity-60 shadow-lg hover:opacity-100"
        title="Open Dev Toolbar"
      >
        DEV
      </button>
    )
  }

  return (
    // biome-ignore lint: DEV-ONLY component — raw colors intentional
    <div className="fixed bottom-12 right-4 z-[9999] bg-gray-900 text-white text-xs rounded-lg shadow-2xl p-3 w-72 border border-yellow-500/50">
      <div className="mb-2 flex items-center justify-between">
        <span className="font-bold text-yellow-400">Dev Toolbar</span>
        <button type="button" onClick={() => setOpen(false)} className="text-gray-400 hover:text-white">
          &#x2715;
        </button>
      </div>

      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <span>Standalone Mock</span>
          <button
            type="button"
            onClick={toggleStandalone}
            className={`rounded px-2 py-0.5 font-bold text-xs ${standalone ? 'bg-red-600' : 'bg-green-600'}`}
          >
            {standalone ? 'ON (mock)' : 'OFF (real API)'}
          </button>
        </div>

        <div className="flex items-center justify-between">
          <span>API Base</span>
          <span className="ml-2 truncate text-gray-400">{apiBase}</span>
        </div>

        <div className="flex items-center justify-between">
          <span>Tauri</span>
          <span className="text-gray-400">{'__TAURI_INTERNALS__' in window ? 'Yes' : 'No'}</span>
        </div>

        <div className="flex items-center justify-between">
          <span>Port</span>
          <span className="text-gray-400">{window.__ONESHIM_WEB_PORT__ ?? 'default'}</span>
        </div>

        <Divider className="border-gray-700" />

        <div className="flex gap-2">
          <button
            type="button"
            onClick={clearStorage}
            className="flex-1 rounded bg-red-800 px-2 py-1 text-xs hover:bg-red-700"
          >
            Clear Storage
          </button>
          <button
            type="button"
            onClick={() => location.reload()}
            className="flex-1 rounded bg-blue-800 px-2 py-1 text-xs hover:bg-blue-700"
          >
            Reload
          </button>
        </div>

        {'__TAURI_INTERNALS__' in window && (
          <div className="flex gap-2">
            <button
              type="button"
              onClick={async () => {
                try {
                  const { invoke } = await import('@tauri-apps/api/core')
                  await invoke('open_devtools')
                } catch {
                  /* fallback */
                }
              }}
              className="flex-1 rounded bg-purple-800 px-2 py-1 text-xs hover:bg-purple-700"
            >
              Main DevTools
            </button>
            <button
              type="button"
              onClick={async () => {
                try {
                  const { invoke } = await import('@tauri-apps/api/core')
                  await invoke('open_devtools', { label: 'tracking-panel' })
                } catch {
                  /* fallback */
                }
              }}
              className="flex-1 rounded bg-indigo-800 px-2 py-1 text-xs hover:bg-indigo-700"
            >
              Panel DevTools
            </button>
          </div>
        )}
      </div>
    </div>
  )
}
