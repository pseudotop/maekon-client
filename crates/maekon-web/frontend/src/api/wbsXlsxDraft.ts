/** Receipt-only WBS XLSX IPC surface (#11120). */

type LocalXlsxEvidence = {
  receipt_id: string
  assignment_receipt_id: string
  assignment_id: string
  mapping_id: string
  output_path: string
  artifact_sha256: string
  template_structure_hash: string
  mapping_content_hash: string
  approved_template_hash: string
  assignment_hash: string
  source_snapshot_hash: string
  row_count: number
  escaped_cell_count: number
}

export type GenerateWbsXlsxResult =
  | { outcome: 'cancelled' }
  | { outcome: 'unverified'; reason: 'generation' }
  | { outcome: 'unverified'; reason: 'upload'; local: LocalXlsxEvidence }
  | {
      outcome: 'error'
      error_kind: 'api' | 'local_receipt' | 'unsafe_workbook'
      code: string
    }
  | {
      outcome: 'gate_rejected'
      receipt_id: string
      reason_code: string
      receipt_upload_pending: boolean
    }
  | {
      outcome: 'header_drift'
      receipt_id: string
      first_mismatch_column: string
      receipt_upload_pending: boolean
    }
  | (LocalXlsxEvidence & {
      outcome: 'produced'
      receipt_upload_pending: boolean
    })

export async function generateWbsXlsxDraft(assignmentReceiptId: string): Promise<GenerateWbsXlsxResult> {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const response = await invoke<unknown>('generate_tmd_xlsx', { assignmentReceiptId })
    return decodeResult(response, assignmentReceiptId)
  } catch (error) {
    const code = readErrorCode(error)
    return {
      outcome: 'error',
      error_kind: code.startsWith('storage.')
        ? 'local_receipt'
        : code === 'validation.invalid_arguments'
          ? 'unsafe_workbook'
          : 'api',
      code,
    }
  }
}

function decodeResult(response: unknown, assignmentReceiptId: string): GenerateWbsXlsxResult {
  const unverified = { outcome: 'unverified', reason: 'generation' } as const
  if (!response || typeof response !== 'object' || Array.isArray(response)) return unverified
  const value = response as Record<string, unknown>
  if (value.outcome === 'cancelled') return { outcome: 'cancelled' }
  if (value.outcome === 'gate_rejected') {
    if (
      !nonEmptyString(value.receipt_id) ||
      !nonEmptyString(value.reason_code) ||
      typeof value.receipt_upload_pending !== 'boolean'
    )
      return unverified
    return {
      outcome: 'gate_rejected',
      receipt_id: value.receipt_id,
      reason_code: value.reason_code,
      receipt_upload_pending: value.receipt_upload_pending,
    }
  }
  if (value.outcome === 'header_drift') {
    if (
      !nonEmptyString(value.receipt_id) ||
      !nonEmptyString(value.first_mismatch_column) ||
      typeof value.receipt_upload_pending !== 'boolean'
    )
      return unverified
    return {
      outcome: 'header_drift',
      receipt_id: value.receipt_id,
      first_mismatch_column: value.first_mismatch_column,
      receipt_upload_pending: value.receipt_upload_pending,
    }
  }
  if (value.outcome !== 'produced' || !hasLocalEvidence(value) || value.assignment_receipt_id !== assignmentReceiptId)
    return unverified
  const local: LocalXlsxEvidence = {
    receipt_id: value.receipt_id,
    assignment_receipt_id: value.assignment_receipt_id,
    assignment_id: value.assignment_id,
    mapping_id: value.mapping_id,
    output_path: value.output_path,
    artifact_sha256: value.artifact_sha256,
    template_structure_hash: value.template_structure_hash,
    mapping_content_hash: value.mapping_content_hash,
    approved_template_hash: value.approved_template_hash,
    assignment_hash: value.assignment_hash,
    source_snapshot_hash: value.source_snapshot_hash,
    row_count: value.row_count,
    escaped_cell_count: value.escaped_cell_count,
  }
  // Local creation and durable receipt evidence do not establish server upload.
  if (typeof value.receipt_upload_pending !== 'boolean') return { outcome: 'unverified', reason: 'upload', local }
  return { outcome: 'produced', ...local, receipt_upload_pending: value.receipt_upload_pending }
}

function hasLocalEvidence(value: Record<string, unknown>): value is Record<string, unknown> & LocalXlsxEvidence {
  return (
    nonEmptyString(value.receipt_id) &&
    nonEmptyString(value.assignment_receipt_id) &&
    nonEmptyString(value.assignment_id) &&
    nonEmptyString(value.mapping_id) &&
    nonEmptyString(value.output_path) &&
    sha256(value.artifact_sha256) &&
    sha256(value.template_structure_hash) &&
    sha256(value.mapping_content_hash) &&
    sha256(value.approved_template_hash) &&
    sha256(value.assignment_hash) &&
    sha256(value.source_snapshot_hash) &&
    count(value.row_count) &&
    count(value.escaped_cell_count)
  )
}

function nonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0
}

function sha256(value: unknown): value is string {
  return typeof value === 'string' && value.length === 64 && /^[0-9a-f]{64}$/iu.test(value)
}

function count(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
}

function readErrorCode(error: unknown): string {
  if (error && typeof error === 'object' && 'code' in error && typeof error.code === 'string') {
    return error.code
  }
  return 'service.unavailable'
}
