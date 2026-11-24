import { beforeEach, describe, expect, it, vi } from 'vitest'
import { generateWbsXlsxDraft } from '../wbsXlsxDraft'

const invoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({ invoke }))

function produced() {
  return {
    outcome: 'produced',
    receipt_id: 'wxr-1',
    assignment_receipt_id: 'ercv-1',
    assignment_id: 'wfa-1',
    mapping_id: 'map-1',
    output_path: '/tmp/draft.xlsx',
    artifact_sha256: 'a'.repeat(64),
    template_structure_hash: 'b'.repeat(64),
    mapping_content_hash: 'c'.repeat(64),
    approved_template_hash: 'd'.repeat(64),
    assignment_hash: 'e'.repeat(64),
    source_snapshot_hash: 'f'.repeat(64),
    row_count: 4,
    escaped_cell_count: 1,
    receipt_upload_pending: false,
  }
}

describe('receipt-only WBS XLSX IPC', () => {
  beforeEach(() => {
    invoke.mockReset()
  })

  it('passes only the assignment receipt to native code', async () => {
    invoke.mockResolvedValue({ outcome: 'cancelled' })

    await generateWbsXlsxDraft('ercv-1')

    expect(invoke).toHaveBeenCalledWith('generate_tmd_xlsx', {
      assignmentReceiptId: 'ercv-1',
    })
    const args = invoke.mock.calls[0]?.[1] as Record<string, unknown>
    for (const forbidden of [
      'organizationId',
      'mappingId',
      'assignmentId',
      'inputPath',
      'outputPath',
      'bearer',
      'token',
    ]) {
      expect(args).not.toHaveProperty(forbidden)
    }
  })

  it.each([
    ['validation.invalid_arguments', 'unsafe_workbook'],
    ['storage.write_failed', 'local_receipt'],
    ['service.unavailable', 'api'],
  ] as const)('normalizes %s rejection as %s', async (code, errorKind) => {
    invoke.mockRejectedValueOnce({ code, message: 'redacted by the adapter' })

    await expect(generateWbsXlsxDraft('ercv-1')).resolves.toEqual({
      outcome: 'error',
      error_kind: errorKind,
      code,
    })
  })

  it.each([
    null,
    undefined,
    [],
    'produced',
    0,
    {},
    { outcome: 'unverified' },
    { outcome: 'produced' },
    { outcome: 'error', code: 'storage.failed' },
  ])('keeps malformed native response %j unverified', async (response) => {
    invoke.mockResolvedValue(response)
    await expect(generateWbsXlsxDraft('ercv-1')).resolves.toEqual({ outcome: 'unverified', reason: 'generation' })
    expect(invoke).toHaveBeenCalledOnce()
  })

  it.each([
    ['receipt_id', ''],
    ['assignment_id', '  '],
    ['mapping_id', null],
    ['output_path', undefined],
    ['assignment_receipt_id', 'ercv-other'],
    ['assignment_receipt_id', ' ercv-1'],
    ['artifact_sha256', 'a'.repeat(63)],
    ['template_structure_hash', 'g'.repeat(64)],
    ['mapping_content_hash', 123],
    ['approved_template_hash', null],
    ['assignment_hash', 'a'.repeat(65)],
    ['source_snapshot_hash', `${'a'.repeat(64)}\n`],
    ['row_count', -1],
    ['row_count', 1.5],
    ['row_count', Number.MAX_SAFE_INTEGER + 1],
    ['row_count', '4'],
    ['escaped_cell_count', Number.NaN],
    ['escaped_cell_count', Number.POSITIVE_INFINITY],
  ])('does not claim creation with invalid %s = %j', async (key, value) => {
    invoke.mockResolvedValue({ ...produced(), [key as string]: value })
    await expect(generateWbsXlsxDraft('ercv-1')).resolves.toEqual({ outcome: 'unverified', reason: 'generation' })
  })

  it.each([false, true])('accepts complete bound evidence with explicit pending=%s', async (pending) => {
    const response = {
      ...produced(),
      row_count: 0,
      escaped_cell_count: 0,
      artifact_sha256: 'A'.repeat(64),
      receipt_upload_pending: pending,
    }
    invoke.mockResolvedValue(response)
    await expect(generateWbsXlsxDraft('ercv-1')).resolves.toEqual(response)
  })

  it.each([
    undefined,
    null,
    0,
    1,
    '',
    'false',
    'true',
  ])('retains local evidence but never invents upload status %j', async (pending) => {
    const { outcome: _outcome, receipt_upload_pending: _pending, ...local } = produced()
    invoke.mockResolvedValue({ ...produced(), receipt_upload_pending: pending })
    await expect(generateWbsXlsxDraft('ercv-1')).resolves.toEqual({ outcome: 'unverified', reason: 'upload', local })
    expect(invoke).toHaveBeenCalledOnce()
  })

  it.each([
    { outcome: 'cancelled' },
    { outcome: 'gate_rejected', receipt_id: 'wxr-gate', reason_code: 'template_stale', receipt_upload_pending: false },
    { outcome: 'header_drift', receipt_id: 'wxr-drift', first_mismatch_column: 'WBS', receipt_upload_pending: true },
  ])('accepts the native terminal variant $outcome', async (response) => {
    invoke.mockResolvedValue(response)
    await expect(generateWbsXlsxDraft('ercv-1')).resolves.toEqual(response)
  })

  it.each([
    { outcome: 'gate_rejected', reason_code: 'template_stale', receipt_upload_pending: false },
    { outcome: 'gate_rejected', receipt_id: 'wxr-gate', reason_code: '', receipt_upload_pending: true },
    { outcome: 'header_drift', receipt_id: 'wxr-drift', first_mismatch_column: 'WBS' },
    { outcome: 'header_drift', receipt_id: 'wxr-drift', first_mismatch_column: 3, receipt_upload_pending: false },
  ])('does not convert an incomplete $outcome result into a known failure', async (response) => {
    invoke.mockResolvedValue(response)
    await expect(generateWbsXlsxDraft('ercv-1')).resolves.toEqual({ outcome: 'unverified', reason: 'generation' })
  })
})
