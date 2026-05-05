export type Transport = 'subprocess' | 'http_api' | 'local_llm'
export type SessionState = 'starting' | 'active' | 'idle' | 'recovering' | 'failed' | 'terminated'

export interface SessionConfig {
  transport: Transport
  surface_id?: string
  model?: string
  system_prompt?: string
  tools_enabled: boolean
}

export interface SessionInfo {
  session_id: string
  provider_name: string
  model: string
  state: SessionState
  transport: Transport
  created_at: string
  last_active: string
  turn_count: number
  title?: string
}

export type OutboundMessage =
  | { type: 'text'; content: string; done: boolean }
  | { type: 'thinking'; content: string; done: boolean }
  | {
      type: 'result'
      content: string
      done: boolean
      usage?: { input_tokens: number; output_tokens: number }
    }
  | {
      type: 'tool_use'
      tool: string
      status: 'started' | 'completed' | 'failed'
      input?: unknown
      result?: string
    }
  | {
      type: 'tool_call_delta'
      index: number
      id: string
      name: string
      arguments_chunk: string
    }
  | { type: 'error'; code: string; message: string; retryable: boolean }
  | { type: 'control'; action: string }

export interface ChatMessage {
  role: 'user' | 'assistant' | 'system'
  content: string
  timestamp: string
  streaming?: boolean
  thinking?: { content: string; done: boolean }
  tool_use?: { tool: string; status: string; input?: Record<string, unknown>; result?: string }
  tool_call_delta?: { index: number; id: string; name: string; arguments: string }
  usage?: { input_tokens: number; output_tokens: number }
  error?: { code: string; message: string; retryable: boolean }
}

export interface MessageRecord {
  id: number | null
  session_id: string
  role: string
  content: string
  thinking: string | null
  tool_use: string | null
  usage_input: number | null
  usage_output: number | null
  created_at: string
  seq: number
}

export type AttachmentPayload =
  | { kind: 'image'; mime: string; data?: string | null; path?: string | null }
  | { kind: 'file'; path: string; mime?: string | null; data?: string | null }

export interface ToolDefinitionPayload {
  name: string
  description: string
  endpoint: string
  method?: string
  input_schema?: unknown
}

export interface ParsedJsonResult<T> {
  value: T | undefined
  error: boolean
}
