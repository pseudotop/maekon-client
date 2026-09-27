/** Receipt-only WBS XLSX IPC surface (#11120). */

export type GenerateWbsXlsxResult =
  | { outcome: 'cancelled' }
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
  | {
      outcome: 'produced'
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
      receipt_upload_pending: boolean
    }

export async function generateWbsXlsxDraft(assignmentReceiptId: string): Promise<GenerateWbsXlsxResult> {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    return await invoke<GenerateWbsXlsxResult>('generate_tmd_xlsx', { assignmentReceiptId })
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

function readErrorCode(error: unknown): string {
  if (error && typeof error === 'object' && 'code' in error && typeof error.code === 'string') {
    return error.code
  }
  return 'service.unavailable'
}
