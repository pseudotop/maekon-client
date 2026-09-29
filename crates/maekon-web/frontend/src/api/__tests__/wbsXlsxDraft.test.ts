import { beforeEach, describe, expect, it, vi } from 'vitest'
import { generateWbsXlsxDraft } from '../wbsXlsxDraft'

const invoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({ invoke }))

describe('receipt-only WBS XLSX IPC', () => {
  beforeEach(() => invoke.mockReset())

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
})
