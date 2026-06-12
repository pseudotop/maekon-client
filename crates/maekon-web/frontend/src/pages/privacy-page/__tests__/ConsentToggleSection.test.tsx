/**
 * ConsentToggleSection 행위 테스트 (#4629 A.2 task 2).
 *
 * 카피(문구)가 아니라 *행위* — 즉 어떤 IPC invoke 가 어떤 인자로 호출되는지를 검증한다.
 * (실제 i18n 키 문자열은 Task 3 에서 추가되며, 키 미존재 시 i18next 는 키 자체를
 * 반환하므로 t() 가 키를 그대로 돌려줘도 green 이다.)
 *
 * 핵심 계약(Task 1 검증 기준):
 *   - get_consent 는 status 가 Expired/UpdateRequired 여도 RAW 부여 권한을 반환한다.
 *     따라서 "모니터링 ON" 판정은 status === 'Valid' AND 필드 로만 한다.
 *   - set_consent 는 레코드를 통째로 교체하므로, 모든 핸들러는 현재 권한 전체를
 *     spread 한 뒤 대상 필드만 뒤집어 전송한다 (별도 opt-in 유실 방지).
 */

import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import type { ConsentPermissions, ConsentSnapshot, ConsentStatus } from '../../../api/contracts'
// 테스트 i18n 은 en 로 해석된다(fallbackLng='en'). Task 3 이후 키는 실제 카피로
// 해석되므로, 키 문자열 대신 en.json 의 해석된 카피를 직접 참조한다(문구 변경에도
// 키 경로만 유지되면 테스트가 자동 정합).
import en from '../../../i18n/locales/en.json'
import ConsentToggleSection from '../ConsentToggleSection'

// 14개 권한 전부 false 인 기준 권한 집합.
const ALL_FALSE: ConsentPermissions = {
  screen_capture: false,
  ocr_processing: false,
  telemetry: false,
  process_monitoring: false,
  input_activity: false,
  window_title_collection: false,
  app_usage_analytics: false,
  clipboard_monitoring: false,
  file_access_monitoring: false,
  activity_pattern_learning: false,
  cross_device_sync: false,
  full_text_extraction: false,
  memory_graph_enrichment: false,
  microphone: false,
}

function snapshot(status: ConsentStatus, overrides: Partial<ConsentPermissions> = {}): ConsentSnapshot {
  return { status, permissions: { ...ALL_FALSE, ...overrides } }
}

/**
 * get_consent 는 항상 주어진 스냅샷을 반환하고, set_consent / withdraw_consent 는
 * 전달된 인자를 spy 로 기록한 뒤 적당한 스냅샷을 반환하도록 IPC 를 모킹한다.
 */
function mockConsentIpc(initial: ConsentSnapshot) {
  const setSpy = vi.fn()
  const withdrawSpy = vi.fn()
  mockIPC((cmd, args) => {
    if (cmd === 'get_consent') {
      return initial
    }
    if (cmd === 'set_consent') {
      setSpy(args)
      const permissions = (args as { permissions: ConsentPermissions }).permissions
      return { status: 'Valid', permissions } satisfies ConsentSnapshot
    }
    if (cmd === 'withdraw_consent') {
      withdrawSpy(args)
      return snapshot('NotGranted')
    }
    return undefined
  })
  return { setSpy, withdrawSpy }
}

describe('ConsentToggleSection', () => {
  afterEach(() => clearMocks())

  it('mounts → calls get_consent and reflects a Valid+screen_capture snapshot as monitoring ON', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const toggle = await screen.findByTestId('consent-monitoring-toggle')
    await waitFor(() => expect(toggle).toBeChecked())
  })

  it('toggling monitoring ON from a NotGranted snapshot → set_consent with the 6 master fields true (others preserved)', async () => {
    // ocr_processing 는 별도로 이미 켜져 있다고 가정 → spread 로 보존되어야 한다.
    const { setSpy } = mockConsentIpc(snapshot('NotGranted', { ocr_processing: true }))

    renderWithProviders(<ConsentToggleSection />)

    const toggle = await screen.findByTestId('consent-monitoring-toggle')
    await waitFor(() => expect(toggle).not.toBeChecked())
    fireEvent.click(toggle)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    // 6개 마스터 필드 ON
    expect(sent.screen_capture).toBe(true)
    expect(sent.window_title_collection).toBe(true)
    expect(sent.app_usage_analytics).toBe(true)
    expect(sent.process_monitoring).toBe(true)
    expect(sent.input_activity).toBe(true)
    expect(sent.telemetry).toBe(true)
    // 별도 opt-in 보존 (spread)
    expect(sent.ocr_processing).toBe(true)
    // 고감도 opt-in 은 건드리지 않는다
    expect(sent.clipboard_monitoring).toBe(false)
    expect(sent.file_access_monitoring).toBe(false)
  })

  it('toggling the microphone opt-in → set_consent flips microphone, all other fields preserved (full-spread)', async () => {
    // 모니터링은 이미 Valid 로 켜져 있고 클립보드도 별도로 켜진 상태에서 마이크만 추가로 켠다.
    const { setSpy } = mockConsentIpc(
      snapshot('Valid', {
        screen_capture: true,
        window_title_collection: true,
        app_usage_analytics: true,
        process_monitoring: true,
        input_activity: true,
        telemetry: true,
        clipboard_monitoring: true,
        file_access_monitoring: true,
      }),
    )

    renderWithProviders(<ConsentToggleSection />)

    const microphone = await screen.findByTestId('consent-microphone-toggle')
    await waitFor(() => expect(microphone).not.toBeChecked())
    fireEvent.click(microphone)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.microphone).toBe(true)
    // 다른 모든 필드는 spread 로 보존된다.
    expect(sent.screen_capture).toBe(true)
    expect(sent.window_title_collection).toBe(true)
    expect(sent.telemetry).toBe(true)
    expect(sent.clipboard_monitoring).toBe(true)
    expect(sent.file_access_monitoring).toBe(true)
  })

  it('an Expired snapshot reads the microphone toggle as OFF even though microphone is true (status gating)', async () => {
    mockConsentIpc(snapshot('Expired', { microphone: true }))

    renderWithProviders(<ConsentToggleSection />)

    const microphone = await screen.findByTestId('consent-microphone-toggle')
    // status !== 'Valid' 이므로 RAW 권한이 true 여도 토글은 OFF (fail-closed 표시).
    await waitFor(() => expect(microphone).not.toBeChecked())
  })

  it('renders the microphone cloud-egress disclosure as a visible warning (role="alert")', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    // disclosure 는 Alert variant="warning" → role="alert" 로 가시 경고로 렌더된다.
    // (정적 렌더 role="alert" 는 reliably announced 되지 않으므로 — WAI-ARIA: load 이후 삽입
    // 시에만 자동 고지 — "고지(announced)" 가 아니라 가시 경고 + 토글-연결(아래 테스트)로 본다.)
    const disclosure = await screen.findByText(en.privacy.consent.microphone.disclosure)
    expect(disclosure).toBeInTheDocument()
    // 가장 가까운 role="alert" 컨테이너 안에 렌더된다(Alert 가 warning→role=alert 로 매핑).
    expect(disclosure.closest('[role="alert"]')).not.toBeNull()
  })

  // a11y: 정적 role="alert" 는 reliably announced 되지 않으므로(WAI-ARIA), disclosure 를
  // 마이크 토글과 aria-describedby 로 *연결*한다 — 스크린리더가 마이크 토글을 읽을 때
  // 클라우드 유출 disclosure 도 함께 읽도록. (deep review POLISH 2.)
  it('associates the cloud-egress disclosure with the mic toggle via aria-describedby', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const microphone = await screen.findByTestId('consent-microphone-toggle')
    // 마이크 체크박스는 disclosure 를 가리키는 aria-describedby 를 가져야 한다.
    const describedBy = microphone.getAttribute('aria-describedby')
    expect(describedBy).toBeTruthy()
    // 그 id 로 가리켜지는 노드는 disclosure 카피를 담은 (role="alert") 요소여야 한다.
    const described = document.getElementById(describedBy as string)
    expect(described).not.toBeNull()
    expect(described).toHaveTextContent(en.privacy.consent.microphone.disclosure)
    expect(described?.getAttribute('role')).toBe('alert')
  })

  it('toggling clipboard opt-in → set_consent flips clipboard_monitoring/file_access_monitoring, other fields preserved', async () => {
    // 모니터링은 이미 Valid 로 켜져 있는 상태에서 클립보드만 추가로 켠다.
    const { setSpy } = mockConsentIpc(
      snapshot('Valid', {
        screen_capture: true,
        window_title_collection: true,
        app_usage_analytics: true,
        process_monitoring: true,
        input_activity: true,
        telemetry: true,
      }),
    )

    renderWithProviders(<ConsentToggleSection />)

    const clipboard = await screen.findByTestId('consent-clipboard-toggle')
    await waitFor(() => expect(clipboard).not.toBeChecked())
    fireEvent.click(clipboard)

    await waitFor(() => expect(setSpy).toHaveBeenCalledTimes(1))
    const sent = (setSpy.mock.calls[0][0] as { permissions: ConsentPermissions }).permissions
    expect(sent.clipboard_monitoring).toBe(true)
    expect(sent.file_access_monitoring).toBe(true)
    // 모니터링 마스터 필드는 유실되지 않는다 (spread).
    expect(sent.screen_capture).toBe(true)
    expect(sent.window_title_collection).toBe(true)
    expect(sent.telemetry).toBe(true)
  })

  it('withdraw action → calls withdraw_consent', async () => {
    const { withdrawSpy } = mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    // 철회는 확인 모달을 거친다: 트리거 → 모달의 확인 버튼.
    // 확인 버튼은 ConfirmModal 이 confirmText(해석된 카피)로 라벨링한 danger 버튼.
    const trigger = await screen.findByTestId('consent-withdraw-trigger')
    fireEvent.click(trigger)
    const confirm = await screen.findByRole('button', {
      name: en.privacy.consent.withdraw.confirmButton,
    })
    fireEvent.click(confirm)

    await waitFor(() => expect(withdrawSpy).toHaveBeenCalledTimes(1))
  })

  it('an Expired snapshot renders a visible warning', async () => {
    mockConsentIpc(snapshot('Expired', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    // Expired 경고는 Alert variant="warning" → role="alert" 로 노출된다.
    const warning = await screen.findByText(en.privacy.consent.status.expiredWarning)
    expect(warning).toBeInTheDocument()
  })

  it('an Expired snapshot reads the monitoring toggle as OFF even though screen_capture is true (status gating)', async () => {
    mockConsentIpc(snapshot('Expired', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    const toggle = await screen.findByTestId('consent-monitoring-toggle')
    // status !== 'Valid' 이므로 RAW 권한이 true 여도 토글은 OFF (fail-closed 표시).
    await waitFor(() => expect(toggle).not.toBeChecked())
  })

  // F6 (a11y): 각 동의 토글은 가시 라벨과 동일한 접근성 이름을 가져야 한다 — 스크린리더가
  // "checkbox, unchecked" 가 아니라 라벨 카피로 토글의 목적을 읽을 수 있어야 한다.
  it('exposes each consent toggle to assistive tech with its visible label as the accessible name', async () => {
    mockConsentIpc(snapshot('Valid', { screen_capture: true }))

    renderWithProviders(<ConsentToggleSection />)

    // 역할(role=checkbox) + 접근성 이름(name=가시 라벨 카피)으로 각 토글이 해석되어야 한다.
    const monitoring = await screen.findByRole('checkbox', {
      name: en.privacy.consent.monitoring.label,
    })
    const clipboard = await screen.findByRole('checkbox', {
      name: en.privacy.consent.clipboard.label,
    })
    const microphone = await screen.findByRole('checkbox', {
      name: en.privacy.consent.microphone.label,
    })
    expect(monitoring).toBeInTheDocument()
    expect(clipboard).toBeInTheDocument()
    expect(microphone).toBeInTheDocument()
    // 접근성 이름으로 찾은 컨트롤이 testId 로 찾은 것과 동일한 노드인지 확인(별도 input 아님).
    expect(monitoring).toBe(screen.getByTestId('consent-monitoring-toggle'))
    expect(clipboard).toBe(screen.getByTestId('consent-clipboard-toggle'))
    expect(microphone).toBe(screen.getByTestId('consent-microphone-toggle'))
  })

  // F7 (demo/standalone): Tauri 부재 시 get_consent 가 reject → useQuery 가 error 상태가 되고
  // data 는 undefined 로 남는다. 영구 로딩 카드 대신 "사용 불가" 안내를 렌더해야 한다.
  it('renders an unavailable note (not a stuck loading card) when get_consent rejects', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_consent') {
        throw new Error('window.__TAURI_INTERNALS__ is undefined')
      }
      return undefined
    })

    renderWithProviders(<ConsentToggleSection />)

    // 사용 불가 안내가 노출된다.
    const unavailable = await screen.findByText(en.privacy.consent.unavailable)
    expect(unavailable).toBeInTheDocument()
    // 영구 로딩 카드는 더 이상 보이지 않아야 한다.
    expect(screen.queryByText(en.privacy.consent.loading)).not.toBeInTheDocument()
  })
})
