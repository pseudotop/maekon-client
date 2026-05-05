import type { ProviderSurfaceSpec } from '../../api/contracts'
import { sortProviderSurfaces } from '../../features/providerSurfaces'
import type { ChatMessage, MessageRecord, ParsedJsonResult, ToolDefinitionPayload } from './types'

export function recordToChat(r: MessageRecord): ChatMessage {
  return {
    role: r.role as ChatMessage['role'],
    content: r.content,
    timestamp: r.created_at,
    thinking: r.thinking ? { content: r.thinking, done: true } : undefined,
    tool_use: r.tool_use ? JSON.parse(r.tool_use) : undefined,
    usage:
      r.usage_input != null && r.usage_output != null
        ? { input_tokens: r.usage_input, output_tokens: r.usage_output }
        : undefined,
  }
}

export function parseDataUrl(dataUrl: string): { mime: string; data: string } | null {
  const match = dataUrl.match(/^data:([^;,]+)?(?:;base64)?,(.*)$/)
  if (!match) return null
  const mime = match[1] || 'application/octet-stream'
  const data = match[2] || ''
  return { mime, data }
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function isToolDefinitionPayload(value: unknown): value is ToolDefinitionPayload {
  if (!isRecord(value)) return false
  if (typeof value.name !== 'string') return false
  if (typeof value.description !== 'string') return false
  if (typeof value.endpoint !== 'string') return false
  if (value.method !== undefined && typeof value.method !== 'string') return false
  return true
}

export function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message.trim()) return error.message
  if (typeof error === 'string' && error.trim()) return error
  if (isRecord(error) && typeof error.message === 'string' && error.message.trim()) return error.message
  return fallback
}

export function parseOptionalJsonValue(raw: string): ParsedJsonResult<unknown> {
  const trimmed = raw.trim()
  if (!trimmed) return { value: undefined, error: false }
  try {
    return { value: JSON.parse(trimmed), error: false }
  } catch {
    return { value: undefined, error: true }
  }
}

export function parseOptionalToolDefinitions(raw: string): ParsedJsonResult<ToolDefinitionPayload[]> {
  const parsed = parseOptionalJsonValue(raw)
  if (parsed.error) return { value: undefined, error: true }
  if (parsed.value === undefined) return { value: undefined, error: false }
  if (!Array.isArray(parsed.value) || !parsed.value.every(isToolDefinitionPayload)) {
    return { value: undefined, error: true }
  }
  return { value: parsed.value, error: false }
}

export function filterHttpApiSurfaces(catalog: { surfaces: ProviderSurfaceSpec[] }) {
  return sortProviderSurfaces(
    catalog.surfaces.filter(
      (surface) =>
        surface.supports.llm &&
        surface.execution_kind === 'direct_http' &&
        surface.llm_transport?.auth_scheme !== 'aws_signature_v4',
    ),
  )
}

export async function ipc<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<T>(cmd, args)
}

export function now() {
  return new Date().toISOString()
}
