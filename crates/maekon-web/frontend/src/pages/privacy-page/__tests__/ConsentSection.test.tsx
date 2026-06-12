/**
 * Delete-all 위험 구역 행위 테스트 (#4629 A.2 task 5).
 *
 * design.md §2 / §0.1.d: "모든 데이터 삭제"는 *반드시* withdrawConsent() 를 먼저 호출해
 * 인메모리 current_consent 를 동기적으로 null 화(fail-closed)한 뒤, 그 다음에야 기존
 * deleteAllData() REST 삭제를 수행한다(revoke-first, race-free). withdrawConsent() 가
 * throw 하면 삭제로 진행하지 않는다(철회 없이 조용히 지우지 않는다).
 *
 * 이 테스트는 PrivacyLayout 의 실제 deleteAllMutation 을 (ConsentSection 을 자식 라우트로
 * 렌더해) 그대로 구동한다 — 즉 프로덕션 mutation 배선을 검증한다. client.ts 의
 * withdrawConsent / deleteAllData 를 spy 로 감싸 호출 *순서* 와 reject 단락(short-circuit)을
 * 확인한다. (deleteAllData 는 fetch, withdrawConsent 는 Tauri IPC 를 쓰지만, 두 전송 계층을
 * 가로지르는 순서 단언은 client.ts 함수 spy 한 곳에서 결정적으로 관찰한다.)
 */

import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { Route, Routes } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { DeleteResult, StorageStats } from '../../../api/contracts'
import * as toastModule from '../../../hooks/useToast'
import en from '../../../i18n/locales/en.json'
import ConsentSection from '../ConsentSection'
import PrivacyLayout from '../PrivacyLayout'

// vi.mock 팩토리는 파일 최상단으로 호이스트되므로, 거기서 참조하는 spy/공유 시퀀스는
// vi.hoisted() 로 끌어올려 초기화 순서를 맞춘다(top-level const 참조 시 TDZ 에러 방지).
const { callOrder, withdrawSpy, deleteAllSpy } = vi.hoisted(() => {
  const order: string[] = []
  return {
    callOrder: order,
    withdrawSpy: vi.fn(async () => {
      order.push('withdraw')
      return { status: 'NotGranted', permissions: {} } as never
    }),
    deleteAllSpy: vi.fn(async (): Promise<DeleteResult> => {
      order.push('delete')
      return {
        success: true,
        message: 'deleted',
        events_deleted: 0,
        frames_deleted: 0,
        metrics_deleted: 0,
        process_snapshots_deleted: 0,
        idle_periods_deleted: 0,
      }
    }),
  }
})

// client.ts 는 원본을 유지하되 deleteAllData / withdrawConsent / fetchStorageStats 만 교체한다.
// fetchStorageStats 를 스텁해 레이아웃이 isLoading 게이트를 통과하도록 한다(네트워크 회피).
vi.mock('../../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../api/client')>()
  const stats: StorageStats = {
    db_size_bytes: 0,
    frames_size_bytes: 0,
    total_size_bytes: 0,
    frame_count: 0,
    event_count: 0,
    metric_count: 0,
    oldest_data_date: null,
    newest_data_date: null,
  }
  return {
    ...actual,
    fetchStorageStats: vi.fn(async () => stats),
    deleteAllData: deleteAllSpy,
    withdrawConsent: withdrawSpy,
  }
})

/**
 * PrivacyLayout(부모, 실제 deleteAllMutation 보유) + ConsentSection(자식 index 라우트, danger
 * 버튼 + 컨텍스트 소비) 를 메모리 라우터로 렌더한다. get_capture_status IPC 는 mockIPC 로
 * 응답해 레이아웃 mount effect 가 reject 로 떨어지지 않게 한다.
 */
function renderDangerZone() {
  mockIPC((cmd) => {
    if (cmd === 'get_capture_status') {
      return { paused: false, indicator_visible: true }
    }
    return undefined
  })
  return renderWithProviders(
    <Routes>
      <Route path="/" element={<PrivacyLayout />}>
        <Route index element={<ConsentSection />} />
      </Route>
    </Routes>,
  )
}

/** danger 버튼 클릭 → ConfirmModal 의 "Delete All Data" 확정 버튼 클릭. */
async function triggerDeleteAll() {
  const trigger = await screen.findByTestId('delete-all')
  fireEvent.click(trigger)
  // 트리거 버튼과 모달 확정 버튼이 같은 카피(deleteAllButton)를 쓰므로, alertdialog 안으로
  // 범위를 좁혀 모달의 확정(danger) 버튼만 찾는다.
  const dialog = await screen.findByRole('alertdialog')
  const confirm = within(dialog).getByRole('button', { name: en.privacy.deleteAllButton })
  fireEvent.click(confirm)
}

describe('ConsentSection delete-all (revoke-first)', () => {
  afterEach(() => {
    clearMocks()
    callOrder.length = 0
    withdrawSpy.mockClear()
    deleteAllSpy.mockClear()
    vi.restoreAllMocks()
  })

  it('calls withdraw_consent BEFORE deleteAllData (revoke-first ordering)', async () => {
    renderDangerZone()

    await triggerDeleteAll()

    await waitFor(() => expect(deleteAllSpy).toHaveBeenCalledTimes(1))
    expect(withdrawSpy).toHaveBeenCalledTimes(1)
    // 순서: withdraw 가 delete 보다 앞선 인덱스여야 한다.
    expect(callOrder).toEqual(['withdraw', 'delete'])
    expect(callOrder.indexOf('withdraw')).toBeLessThan(callOrder.indexOf('delete'))
  })

  it('does NOT call deleteAllData and surfaces the error when withdrawConsent rejects', async () => {
    const addToastSpy = vi.spyOn(toastModule, 'addToast')
    withdrawSpy.mockImplementationOnce(async () => {
      callOrder.push('withdraw')
      throw new Error('withdraw failed')
    })

    renderDangerZone()

    await triggerDeleteAll()

    // 에러가 표면화될 때까지 대기(mutation onError → addToast('error', ...)).
    await waitFor(() => expect(addToastSpy).toHaveBeenCalledWith('error', 'withdraw failed'))
    // 철회가 실패했으므로 삭제로 진행하지 않는다(short-circuit).
    expect(deleteAllSpy).not.toHaveBeenCalled()
    expect(callOrder).toEqual(['withdraw'])
  })
})
