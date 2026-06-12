/**
 * ConsentToggleSection — GDPR 동의 부여/철회 컨트롤 (#4629 A.2 task 2).
 *
 * design.md §3 의 동의 토글 섹션. 삭제(우-소거) ConsentSection 의 형제로, 그 위에
 * 배치된다. 모니터링 OFF(향후 수집 중단)와 철회(중단 + 소거 요청)를 구분한다.
 *
 * 핵심 의미론(Task 1 검증 기준):
 *   - get_consent 는 status 가 Expired/UpdateRequired 여도 RAW 부여 권한을 반환한다.
 *     따라서 "모니터링 활성" 판정은 status === 'Valid' AND 필드 로만 한다. Expired/
 *     UpdateRequired 상태는 prominent warning 으로 노출한다(에이전트는 그 상태에서
 *     fail-closed).
 *   - set_consent 는 레코드를 통째로 교체한다. 그래서 모든 핸들러는 현재 권한 전체를
 *     spread 한 뒤 대상 필드만 뒤집어 전송한다(별도 opt-in 유실 방지).
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { getConsent, setConsent, withdrawConsent } from '../../api/client'
import type { ConsentPermissions, ConsentSnapshot, ConsentStatus } from '../../api/contracts'
import { errorMessageFromInvoke } from '../../api/desktop'
import { Alert, Button, Card, CardTitle, Checkbox } from '../../components/ui'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { ConfirmModal } from './PrivacyLayout'

const CONSENT_QUERY_KEY = ['consent'] as const

// 마이크 클라우드-유출 disclosure Alert 의 안정적 id. 마이크 토글의 Checkbox 가
// aria-describedby 로 이 id 를 가리켜, 스크린리더가 토글을 읽을 때 disclosure 도 읽도록 한다.
const MICROPHONE_DISCLOSURE_ID = 'consent-microphone-disclosure'

// 모니터링 번들을 구성하는 6개 마스터 필드. ON 이면 모두 true, OFF 면 모두 false.
const MONITORING_FIELDS = [
  'screen_capture',
  'window_title_collection',
  'app_usage_analytics',
  'process_monitoring',
  'input_activity',
  'telemetry',
] as const satisfies readonly (keyof ConsentPermissions)[]

/** 상태가 Valid 일 때만 "활성"으로 본다. RAW 권한 true 여도 만료/갱신필요면 비활성. */
function isActive(status: ConsentStatus, granted: boolean): boolean {
  return status === 'Valid' && granted
}

/**
 * 동의 상태에 따라 열거형 토글 한 줄을 그린다. 설명 영역은 ON 시 수집되는 항목 전체를
 * 열거한다(i18n 키 기반). 토글 컨트롤 자체는 Checkbox 프리미티브를 재사용한다.
 */
interface EnumeratedToggleProps {
  testId: string
  label: string
  description: string
  collectedTitle: string
  collected: string[]
  checked: boolean
  disabled: boolean
  onChange: (checked: boolean) => void
  /**
   * 이 토글에만 국한된 OS 권한 안내(예: 마이크). 섹션 레벨의 화면-녹화 osNote(:255)와
   * 달리, 토글 블록 안에 per-toggle <p> 로 렌더되어 어떤 모달리티의 OS 권한인지 명확히 한다.
   */
  osNote?: string
  /**
   * 이 토글 컨트롤에 보충 설명(예: 마이크 클라우드 유출 disclosure)을 aria-describedby 로
   * 연결할 요소의 id. 정적 role="alert" 는 reliably announced 되지 않으므로(WAI-ARIA),
   * 토글을 읽을 때 스크린리더가 보충 경고도 함께 읽도록 명시적으로 연결한다.
   */
  describedById?: string
}

function EnumeratedToggle({
  testId,
  label,
  description,
  collectedTitle,
  collected,
  checked,
  disabled,
  onChange,
  osNote,
  describedById,
}: EnumeratedToggleProps) {
  // 접근성 이름: 가시 라벨 <span> 에 안정적 id 를 부여하고 Checkbox 에 aria-labelledby 로
  // 연결한다. label prop 을 쓰면 Checkbox 가 라벨을 한 번 더 그려 중복되므로 사용하지 않는다.
  const labelId = `${testId}-label`
  return (
    <div className="flex items-start justify-between gap-4 rounded-lg border border-muted bg-surface-inset p-4">
      <div className="min-w-0">
        <span id={labelId} className={cn(typography.label, colors.text.primary)}>
          {label}
        </span>
        <p className="mt-1 text-content-secondary text-sm">{description}</p>
        <p className="mt-2 text-content-tertiary text-xs">{collectedTitle}</p>
        <ul className="mt-1 list-inside list-disc text-content-tertiary text-xs">
          {collected.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
        {osNote && <p className="mt-2 text-content-tertiary text-xs">{osNote}</p>}
      </div>
      <Checkbox
        data-testid={testId}
        aria-labelledby={labelId}
        aria-describedby={describedById}
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-1 shrink-0"
      />
    </div>
  )
}

export default function ConsentToggleSection() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [showWithdrawModal, setShowWithdrawModal] = useState(false)

  const {
    data: snapshot,
    isLoading,
    isError,
  } = useQuery<ConsentSnapshot>({
    queryKey: CONSENT_QUERY_KEY,
    queryFn: getConsent,
  })

  const setMutation = useMutation({
    mutationFn: (permissions: ConsentPermissions) => setConsent(permissions),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: CONSENT_QUERY_KEY })
    },
  })

  const withdrawMutation = useMutation({
    mutationFn: () => withdrawConsent(),
    onSuccess: () => {
      setShowWithdrawModal(false)
      queryClient.invalidateQueries({ queryKey: CONSENT_QUERY_KEY })
    },
  })

  // demo/standalone(Tauri 부재) 모드: get_consent 가 reject 되면 영구 로딩 대신 "사용 불가"
  // 안내를 노출한다(다른 섹션과 동일한 graceful degradation). isLoading 보다 먼저 분기한다.
  if (isError) {
    return (
      <Card id="section-consent-toggle" variant="default" padding="lg">
        <CardTitle className="mb-2">{t('privacy.consent.title')}</CardTitle>
        <Alert variant="warning">{t('privacy.consent.unavailable')}</Alert>
      </Card>
    )
  }

  if (isLoading || !snapshot) {
    return (
      <Card id="section-consent-toggle" variant="default" padding="lg">
        <CardTitle className="mb-2">{t('privacy.consent.title')}</CardTitle>
        <p className="text-content-secondary text-sm">{t('privacy.consent.loading')}</p>
      </Card>
    )
  }

  const { status, permissions } = snapshot
  const monitoringOn = isActive(status, permissions.screen_capture)
  // 고감도 opt-in 은 두 필드 중 하나라도 켜져 있으면 활성으로 본다.
  const clipboardOn = isActive(status, permissions.clipboard_monitoring || permissions.file_access_monitoring)
  const microphoneOn = isActive(status, permissions.microphone)
  const mutating = setMutation.isPending || withdrawMutation.isPending

  const statusLabel: Record<ConsentStatus, string> = {
    Valid: t('privacy.consent.status.valid'),
    NotGranted: t('privacy.consent.status.notGranted'),
    Expired: t('privacy.consent.status.expired'),
    UpdateRequired: t('privacy.consent.status.updateRequired'),
  }

  const handleMonitoring = (next: boolean) => {
    // 현재 권한 전체를 보존(spread)하고 6개 마스터 필드만 일괄 설정한다.
    const updated: ConsentPermissions = { ...permissions }
    for (const field of MONITORING_FIELDS) {
      updated[field] = next
    }
    setMutation.mutate(updated)
  }

  const handleClipboard = (next: boolean) => {
    setMutation.mutate({
      ...permissions,
      clipboard_monitoring: next,
      file_access_monitoring: next,
    })
  }

  const handleMicrophone = (next: boolean) => {
    // 현재 권한 전체를 보존(spread)하고 microphone 만 뒤집는다(다른 opt-in 유실 방지).
    setMutation.mutate({ ...permissions, microphone: next })
  }

  const lastError = setMutation.isError
    ? errorMessageFromInvoke(setMutation.error)
    : withdrawMutation.isError
      ? errorMessageFromInvoke(withdrawMutation.error)
      : null

  return (
    <Card id="section-consent-toggle" variant="default" padding="lg">
      <CardTitle className="mb-2">{t('privacy.consent.title')}</CardTitle>
      <p className="mb-4 text-content-secondary text-sm">{t('privacy.consent.subtitle')}</p>

      {/* 상태 라인 */}
      <p className="mb-4 text-content-strong text-sm" data-testid="consent-status">
        {t('privacy.consent.status.label')}: {statusLabel[status]}
      </p>

      {/* Expired / UpdateRequired → prominent warning (에이전트 fail-closed) */}
      {status === 'Expired' && (
        <Alert variant="warning" className="mb-4" title={t('privacy.consent.status.expiredTitle')}>
          {t('privacy.consent.status.expiredWarning')}
        </Alert>
      )}
      {status === 'UpdateRequired' && (
        <Alert variant="warning" className="mb-4" title={t('privacy.consent.status.updateRequiredTitle')}>
          {t('privacy.consent.status.updateRequiredWarning')}
        </Alert>
      )}

      {/* IPC 에러 표시 */}
      {lastError && (
        <Alert variant="error" className="mb-4" title={t('privacy.consent.actionFailed')}>
          {lastError}
        </Alert>
      )}

      <div className="space-y-4">
        {/* 모니터링 번들 토글 */}
        <EnumeratedToggle
          testId="consent-monitoring-toggle"
          label={t('privacy.consent.monitoring.label')}
          description={t('privacy.consent.monitoring.description')}
          collectedTitle={t('privacy.consent.monitoring.collectedTitle')}
          collected={[
            t('privacy.consent.monitoring.collected.screenFrames'),
            t('privacy.consent.monitoring.collected.windowTitles'),
            t('privacy.consent.monitoring.collected.appUsage'),
            t('privacy.consent.monitoring.collected.inputActivity'),
            t('privacy.consent.monitoring.collected.activeProcesses'),
            t('privacy.consent.monitoring.collected.systemMetrics'),
            t('privacy.consent.monitoring.collected.activitySegments'),
            t('privacy.consent.monitoring.collected.focusMetrics'),
            t('privacy.consent.monitoring.collected.digests'),
            t('privacy.consent.monitoring.collected.memoryGraph'),
            t('privacy.consent.monitoring.collected.embeddings'),
            t('privacy.consent.monitoring.collected.guiInteractions'),
          ]}
          checked={monitoringOn}
          disabled={mutating}
          onChange={handleMonitoring}
        />

        {/* 고감도 클립보드 / 파일 접근 opt-in (기본 off) */}
        <EnumeratedToggle
          testId="consent-clipboard-toggle"
          label={t('privacy.consent.clipboard.label')}
          description={t('privacy.consent.clipboard.description')}
          collectedTitle={t('privacy.consent.clipboard.collectedTitle')}
          collected={[
            t('privacy.consent.clipboard.collected.clipboardEvents'),
            t('privacy.consent.clipboard.collected.fileAccessEvents'),
          ]}
          checked={clipboardOn}
          disabled={mutating}
          onChange={handleClipboard}
        />

        {/* 고감도 마이크 opt-in (기본 off). disclosure 는 클라우드 STT 시 원본 오디오가
            제3자에 전송될 수 있음을 *무조건* 고지하는 가시 경고(role=alert)로 토글 바로 아래에
            형제로 배치하고, aria-describedby 로 마이크 토글과 연결한다(정적 role="alert" 는
            reliably announced 되지 않으므로 — WAI-ARIA — 토글을 읽을 때 함께 읽히도록).
            osNote 는 화면-녹화가 아닌 마이크 OS 권한을 가리킨다. */}
        <div className="space-y-2">
          <EnumeratedToggle
            testId="consent-microphone-toggle"
            label={t('privacy.consent.microphone.label')}
            description={t('privacy.consent.microphone.description')}
            collectedTitle={t('privacy.consent.microphone.collectedTitle')}
            collected={[
              t('privacy.consent.microphone.collected.audioCapture'),
              t('privacy.consent.microphone.collected.transcript'),
              t('privacy.consent.microphone.collected.cloudEgress'),
            ]}
            checked={microphoneOn}
            disabled={mutating}
            onChange={handleMicrophone}
            osNote={t('privacy.consent.microphone.osNote')}
            describedById={MICROPHONE_DISCLOSURE_ID}
          />
          <Alert id={MICROPHONE_DISCLOSURE_ID} variant="warning">
            {t('privacy.consent.microphone.disclosure')}
          </Alert>
        </div>
      </div>

      {/* 동의 ≠ OS 화면 접근 안내 (둘 다 필요) */}
      <p className="mt-4 text-content-tertiary text-xs">{t('privacy.consent.osNote')}</p>

      {/* 철회 + 소거 요청 (모니터링 OFF 와 시각적으로 분리) */}
      <div className="mt-6 border-muted border-t pt-4">
        <h3 className={cn(typography.label, 'mb-1 text-content-strong')}>{t('privacy.consent.withdraw.label')}</h3>
        <p className="mb-3 text-content-secondary text-sm">{t('privacy.consent.withdraw.description')}</p>
        <Button
          data-testid="consent-withdraw-trigger"
          variant="danger"
          disabled={mutating || status === 'NotGranted'}
          onClick={() => setShowWithdrawModal(true)}
        >
          {t('privacy.consent.withdraw.button')}
        </Button>
      </div>

      <ConfirmModal
        isOpen={showWithdrawModal}
        title={t('privacy.consent.withdraw.confirmTitle')}
        message={t('privacy.consent.withdraw.confirmMessage')}
        note={t('privacy.consent.withdraw.confirmNote')}
        confirmText={t('privacy.consent.withdraw.confirmButton')}
        isDangerous
        onConfirm={() => withdrawMutation.mutate()}
        onCancel={() => setShowWithdrawModal(false)}
      />
    </Card>
  )
}
