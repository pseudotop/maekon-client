import { fireEvent, screen, waitFor } from '@testing-library/react'
import { useNavigate } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import i18n from '../../../i18n'
import WbsXlsxDraftPage from '../WbsXlsxDraftPage'

const mockGenerateNative = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (command: string, args: unknown) =>
    command === 'generate_tmd_xlsx' ? mockGenerateNative(command, args) : Promise.resolve(undefined),
}))

function renderPage(entry = '/wbs-xlsx-draft?receipt=ercv-1') {
  return renderWithProviders(<WbsXlsxDraftPage />, {
    routerProps: { initialEntries: [entry] },
  })
}

function ReceiptLinkNavigator() {
  const navigate = useNavigate()
  return (
    <button type="button" onClick={() => navigate('/wbs-xlsx-draft?receipt=ercv-linked')}>
      Open receipt link
    </button>
  )
}

function produced() {
  return {
    outcome: 'produced',
    receipt_id: 'wxr-1',
    assignment_receipt_id: 'ercv-1',
    assignment_id: 'wfa-1',
    mapping_id: 'map-1',
    output_path: '/tmp/wbs-wfa-1.xlsx',
    artifact_sha256: 'e'.repeat(64),
    template_structure_hash: 'f'.repeat(64),
    mapping_content_hash: 'c'.repeat(64),
    approved_template_hash: 'd'.repeat(64),
    assignment_hash: 'a'.repeat(64),
    source_snapshot_hash: 'b'.repeat(64),
    row_count: 4,
    escaped_cell_count: 1,
    receipt_upload_pending: false,
  }
}

describe('WbsXlsxDraftPage (#11120)', () => {
  beforeEach(async () => {
    mockGenerateNative.mockReset()
    await i18n.changeLanguage('en')
  })

  it('keeps the synthetic boundary visible and invokes the receipt-only command', async () => {
    mockGenerateNative.mockResolvedValue(produced())
    renderPage()

    expect(screen.getByTestId('wbs-xlsx-synthetic-badge')).toHaveAccessibleName()
    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))

    await screen.findByTestId('wbs-xlsx-produced')
    expect(mockGenerateNative).toHaveBeenCalledWith('generate_tmd_xlsx', { assignmentReceiptId: 'ercv-1' })
    expect(screen.getByText('ercv-1')).toBeInTheDocument()
    expect(screen.getByText('e'.repeat(64))).toBeInTheDocument()
    expect(screen.getByText('f'.repeat(64))).toBeInTheDocument()
    expect(screen.getByText(/also appended to the server|서버에도 추가했습니다/u)).toBeInTheDocument()
    expect(screen.getByTestId('wbs-xlsx-synthetic-badge')).toBeVisible()
  })

  it.each([
    ['unknown native outcome', { outcome: 'unverified' }],
    ['unrecognized native tag', { outcome: 'future_native_outcome' }],
    ['incomplete produced result', { outcome: 'produced' }],
    ['another assignment receipt', { ...produced(), assignment_receipt_id: 'ercv-other' }],
    [
      'missing upload confirmation',
      Object.fromEntries(Object.entries(produced()).filter(([key]) => key !== 'receipt_upload_pending')),
    ],
  ])('does not present success for %s (#13580)', async (_name, response) => {
    mockGenerateNative.mockResolvedValue(response)
    renderPage()

    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))

    const unknown = await screen.findByTestId('wbs-xlsx-unverified')
    expect(unknown).toHaveTextContent(/not verified|확인되지 않았습니다/u)
    expect(screen.queryByTestId('wbs-xlsx-produced')).not.toBeInTheDocument()
    expect(screen.queryByText(/also appended to the server|서버에도 추가했습니다/u)).not.toBeInTheDocument()
    expect(screen.getByTestId('wbs-xlsx-synthetic-badge')).toBeVisible()
    expect(mockGenerateNative).toHaveBeenCalledOnce()
  })

  it('reports native cancellation without presenting a success artifact', async () => {
    mockGenerateNative.mockResolvedValue({ outcome: 'cancelled' })
    renderPage()

    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))

    await screen.findByText(/No file created|파일을 만들지 않았습니다/u)
    expect(screen.queryByTestId('wbs-xlsx-produced')).not.toBeInTheDocument()
    expect(screen.getByTestId('wbs-xlsx-synthetic-badge')).toBeVisible()
  })

  it('clears a previous success when the receipt changes', async () => {
    mockGenerateNative.mockResolvedValue(produced())
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))
    await screen.findByTestId('wbs-xlsx-produced')

    fireEvent.change(screen.getByRole('textbox', { name: /receipt|영수증/u }), {
      target: { value: 'ercv-2' },
    })

    expect(screen.queryByTestId('wbs-xlsx-produced')).not.toBeInTheDocument()
  })

  it('hydrates a receipt link when the already-mounted route query changes', async () => {
    renderWithProviders(
      <>
        <ReceiptLinkNavigator />
        <WbsXlsxDraftPage />
      </>,
      { routerProps: { initialEntries: ['/wbs-xlsx-draft'] } },
    )

    fireEvent.click(screen.getByRole('button', { name: 'Open receipt link' }))

    await waitFor(() => expect(screen.getByRole('textbox', { name: /receipt|영수증/u })).toHaveValue('ercv-linked'))
  })

  it('does not reuse an old success after a receipt link changes in flight', async () => {
    let resolveGeneration: ((value: ReturnType<typeof produced>) => void) | undefined
    mockGenerateNative.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveGeneration = resolve
        }),
    )
    renderWithProviders(
      <>
        <ReceiptLinkNavigator />
        <WbsXlsxDraftPage />
      </>,
      { routerProps: { initialEntries: ['/wbs-xlsx-draft?receipt=ercv-1'] } },
    )
    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))

    fireEvent.click(screen.getByRole('button', { name: 'Open receipt link' }))
    await waitFor(() => expect(screen.getByRole('button', { name: /Preparing|준비 중/u })).toBeDisabled())
    await waitFor(() => expect(mockGenerateNative).toHaveBeenCalledOnce())
    resolveGeneration?.(produced())

    await waitFor(() => expect(screen.getByRole('textbox', { name: /receipt|영수증/u })).toHaveValue('ercv-linked'))
    await waitFor(() => expect(screen.getByRole('button', { name: /template|템플릿/u })).toBeEnabled())
    expect(screen.queryByTestId('wbs-xlsx-produced')).not.toBeInTheDocument()
  })

  it('keeps generation visibly locked while a receipt edit invalidates an in-flight result', async () => {
    let resolveGeneration: ((value: ReturnType<typeof produced>) => void) | undefined
    mockGenerateNative.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveGeneration = resolve
        }),
    )
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))

    fireEvent.change(screen.getByRole('textbox', { name: /receipt|영수증/u }), {
      target: { value: 'ercv-edited' },
    })

    expect(screen.getByRole('button', { name: /Preparing|준비 중/u })).toBeDisabled()
    await waitFor(() => expect(mockGenerateNative).toHaveBeenCalledOnce())
    resolveGeneration?.(produced())
    await waitFor(() => expect(screen.getByRole('button', { name: /template|템플릿/u })).toBeEnabled())
    expect(screen.queryByTestId('wbs-xlsx-produced')).not.toBeInTheDocument()
  })

  it('focuses an error state and keeps the synthetic label visible', async () => {
    mockGenerateNative.mockRejectedValue({ code: 'validation.invalid_arguments' })
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))

    const alert = await screen.findByRole('alert')
    await waitFor(() => expect(alert).toHaveFocus())
    expect(alert).toHaveTextContent(/workbook|워크북/iu)
    expect(screen.getByTestId('wbs-xlsx-synthetic-badge')).toBeVisible()
  })

  it.each([
    [
      {
        outcome: 'gate_rejected',
        receipt_id: 'wxr-gate-1',
        reason_code: 'template_stale',
        receipt_upload_pending: false,
      },
      /template_stale/u,
    ],
    [
      {
        outcome: 'header_drift',
        receipt_id: 'wxr-drift-1',
        first_mismatch_column: 'WBS 코드',
        receipt_upload_pending: false,
      },
      /WBS 코드/u,
    ],
    [
      {
        outcome: 'error',
        error_kind: 'local_receipt',
        code: 'storage.failed',
      },
      /receipt|영수증/u,
    ],
    [
      {
        outcome: 'error',
        error_kind: 'api',
        code: 'service.unavailable',
      },
      /could not be created|만들 수 없습니다/u,
    ],
  ] as const)('keeps each native terminal state distinct from success', async (result, expected) => {
    if (result.outcome === 'error') {
      mockGenerateNative.mockRejectedValue({ code: result.code })
    } else {
      mockGenerateNative.mockResolvedValue(result)
    }
    renderPage()

    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))

    expect(await screen.findByRole('alert')).toHaveTextContent(expected)
    expect(screen.queryByTestId('wbs-xlsx-produced')).not.toBeInTheDocument()
  })

  it('shows a durable local success separately from pending server upload', async () => {
    mockGenerateNative.mockResolvedValue({
      ...produced(),
      receipt_upload_pending: true,
    })
    renderPage()

    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))

    const success = await screen.findByTestId('wbs-xlsx-produced')
    expect(success).toHaveTextContent(/upload remains pending|업로드.*보류/u)
  })

  it.each([null, 0, 'false', 'true'])('does not interpret upload flag %j as uploaded or pending', async (pending) => {
    mockGenerateNative.mockResolvedValue({ ...produced(), receipt_upload_pending: pending })
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))
    const unknown = await screen.findByTestId('wbs-xlsx-unverified')
    expect(unknown).toHaveTextContent('Server upload is not verified')
    expect(unknown).toHaveTextContent('wxr-1')
    expect(unknown).toHaveTextContent('e'.repeat(64))
    expect(screen.queryByTestId('wbs-xlsx-produced')).not.toBeInTheDocument()
    expect(unknown).not.toHaveTextContent(/also appended to the server|upload remains pending|could not be created/u)
    expect(mockGenerateNative).toHaveBeenCalledOnce()
  })

  it.each(['en', 'ko'])('shows truthful unknown creation in %s without claiming file absence', async (language) => {
    await i18n.changeLanguage(language)
    mockGenerateNative.mockResolvedValue({ outcome: 'unexpected' })
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))
    const unknown = await screen.findByTestId('wbs-xlsx-unverified')
    expect(unknown).toHaveTextContent(/A file may exist|파일이 남아 있을 수 있습니다/u)
    expect(unknown).not.toHaveTextContent(
      /No file created|could not be created|파일을 만들지 않았습니다|만들 수 없습니다/u,
    )
    await waitFor(() => expect(unknown).toHaveFocus())
    expect(screen.getByTestId('wbs-xlsx-synthetic-badge')).toBeVisible()
  })

  it.each(['en', 'ko'])('keeps confirmed local evidence separate from unknown upload in %s', async (language) => {
    await i18n.changeLanguage(language)
    mockGenerateNative.mockResolvedValue({ ...produced(), receipt_upload_pending: undefined })
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))
    const unknown = await screen.findByTestId('wbs-xlsx-unverified')
    expect(unknown).toHaveTextContent(/Server upload is not verified|서버 업로드가 확인되지 않았습니다/u)
    expect(unknown).toHaveTextContent('ercv-1')
    expect(unknown).toHaveTextContent('/tmp/wbs-wfa-1.xlsx')
    expect(unknown).toHaveTextContent('wxr-1')
    expect(unknown).not.toHaveTextContent(/could not be created|영수증을 저장하지 못했습니다/u)
    expect(screen.queryByTestId('wbs-xlsx-produced')).not.toBeInTheDocument()
  })

  it('keeps a cancelled old request out of the next receipt and allows one new invocation', async () => {
    let resolveOld: ((value: { outcome: 'cancelled' }) => void) | undefined
    mockGenerateNative.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveOld = resolve
        }),
    )
    renderWithProviders(
      <>
        <ReceiptLinkNavigator />
        <WbsXlsxDraftPage />
      </>,
      { routerProps: { initialEntries: ['/wbs-xlsx-draft?receipt=ercv-1'] } },
    )
    const generate = screen.getByRole('button', { name: /template|템플릿/u })
    fireEvent.click(generate)
    fireEvent.click(generate)
    await waitFor(() => expect(mockGenerateNative).toHaveBeenCalledOnce())
    fireEvent.click(screen.getByRole('button', { name: 'Open receipt link' }))
    resolveOld?.({ outcome: 'cancelled' })
    await waitFor(() => expect(screen.getByRole('button', { name: /template|템플릿/u })).toBeEnabled())
    expect(screen.queryByText(/No file created|파일을 만들지 않았습니다/u)).not.toBeInTheDocument()
    mockGenerateNative.mockResolvedValue({ ...produced(), assignment_receipt_id: 'ercv-linked' })
    fireEvent.click(screen.getByRole('button', { name: /template|템플릿/u }))
    const success = await screen.findByTestId('wbs-xlsx-produced')
    expect(success).toHaveTextContent('ercv-linked')
    expect(mockGenerateNative).toHaveBeenCalledTimes(2)
    expect(mockGenerateNative).toHaveBeenLastCalledWith('generate_tmd_xlsx', { assignmentReceiptId: 'ercv-linked' })
  })
})
