/**
 * ConsentToggleSection — GDPR consent grant/withdrawal control (#4629 A.2 task 2).
 *
 * The consent toggle section from design.md §3. A sibling of the deletion (right-to-erasure)
 * ConsentSection, placed above it. It distinguishes monitoring OFF (stop future collection) from
 * withdrawal (stop + erasure request).
 *
 * Core semantics (Task 1 verification criteria):
 *   - get_consent returns the RAW granted permissions even when status is Expired/UpdateRequired.
 *     Therefore the "monitoring active" decision is based solely on status === 'Valid' AND the field.
 *     Expired/UpdateRequired states are surfaced as a prominent warning (the agent is fail-closed in
 *     that state).
 *   - set_consent replaces the record wholesale. So every handler spreads the entire current set of
 *     permissions and then flips only the target field before sending (to avoid losing other opt-ins).
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { CONSENT_QUERY_KEY, getConsent, setConsent, withdrawConsent } from '../../api/client'
import type { ConsentPermissions, ConsentSnapshot, ConsentStatus } from '../../api/contracts'
import { Alert, Button, Card, CardTitle, Checkbox } from '../../components/ui'
import { invalidateAiReadinessSnapshotCache } from '../../hooks/useAiReadinessSnapshot'
import { addToast } from '../../hooks/useToast'
import { describeIpcError } from '../../i18n/tauriIpcErrors'
import { colors, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { IS_LINUX } from '../../utils/platform'
import { ConfirmModal } from './PrivacyLayout'

// Stable id of the microphone cloud-egress disclosure Alert. The microphone toggle's Checkbox
// points to this id via aria-describedby, so the screen reader also reads the disclosure when it reads the toggle.
const MICROPHONE_DISCLOSURE_ID = 'consent-microphone-disclosure'
const FULL_TEXT_DISCLOSURE_ID = 'consent-full-text-disclosure'
const OCR_PROCESSING_DISCLOSURE_ID = 'consent-ocr-processing-disclosure'
const UNREDACTED_OCR_DISCLOSURE_ID = 'consent-unredacted-external-ocr-disclosure'

// The monitoring bundle driven by the master toggle — all true when ON, all
// false when OFF. #9643 review M3: includes ocr_processing to stay in
// lockstep with the first-run onboarding grant (#9631); without it, turning
// monitoring OFF left the onboarding-granted OCR consent silently on.
const MONITORING_FIELDS = [
  'screen_capture',
  'ocr_processing',
  'window_title_collection',
  'app_usage_analytics',
  'process_monitoring',
  'input_activity',
  'telemetry',
] as const satisfies readonly (keyof ConsentPermissions)[]

/** Treated as "active" only when status is Valid. Inactive if expired/update-required, even when the RAW permission is true. */
function isActive(status: ConsentStatus, granted: boolean): boolean {
  return status === 'Valid' && granted
}

/**
 * Renders one enumerated toggle row according to the consent state. The description area enumerates
 * all items collected when ON (i18n key-based). The toggle control itself reuses the Checkbox primitive.
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
   * OS-permission note scoped to this toggle only (e.g. microphone). Unlike the section-level
   * screen-recording osNote (:255), it renders as a per-toggle <p> inside the toggle block to make
   * clear which modality's OS permission it refers to.
   */
  osNote?: string
  /**
   * Id of the element to associate a supplementary note (e.g. the microphone cloud-egress
   * disclosure) with this toggle control via aria-describedby. Since a static role="alert" is not
   * reliably announced (WAI-ARIA), associate it explicitly so the screen reader also reads the
   * supplementary warning when it reads the toggle.
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
  // Accessible name: give the visible label <span> a stable id and associate it with the Checkbox
  // via aria-labelledby. Don't use the label prop, since it would make the Checkbox render the label
  // a second time (duplicating it).
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

interface ConsentToggleSectionProps {
  onConsentChanged?: () => void
}

export default function ConsentToggleSection({ onConsentChanged }: ConsentToggleSectionProps = {}) {
  const { t, i18n } = useTranslation()
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
      invalidateAiReadinessSnapshotCache()
      onConsentChanged?.()
    },
  })

  const withdrawMutation = useMutation({
    mutationFn: () => withdrawConsent(),
    onSuccess: () => {
      setShowWithdrawModal(false)
      queryClient.invalidateQueries({ queryKey: CONSENT_QUERY_KEY })
      invalidateAiReadinessSnapshotCache()
      onConsentChanged?.()
    },
    // #9505: without this, a withdraw failure (e.g. `storage.unavailable` from
    // `commands/consent.rs::erase_all_local_data`) was completely silent — the
    // confirm modal covers the card's inline error alert, so the user saw an
    // open modal that appeared to ignore the click. Toast renders above the
    // modal; the modal intentionally stays open as the retry affordance (same
    // contract as the sibling PrivacyLayout delete-all path from #9492).
    onError: (error: Error) => {
      addToast(
        'error',
        describeIpcError(error, (key) => t(key), i18n.language),
      )
    },
  })

  // demo/standalone (Tauri-absent) mode: if get_consent rejects, surface an "unavailable" note
  // instead of perpetual loading (the same graceful degradation as other sections). Branch on this
  // before isLoading.
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
  // The high-sensitivity opt-in is considered active if either of the two fields is on.
  const clipboardOn = isActive(status, permissions.clipboard_monitoring || permissions.file_access_monitoring)
  const microphoneOn = isActive(status, permissions.microphone)
  const fullTextOn = isActive(status, permissions.full_text_extraction)
  const ocrProcessingOn = isActive(status, permissions.ocr_processing)
  const unredactedExternalOcrOn = isActive(status, permissions.unredacted_external_ocr)
  // #8686 AC2: these three opt-ins are granted during onboarding (coaching /
  // sync / memory graph) but previously had NO granular revoke — only the
  // nuclear withdraw-everything path removed them.
  const patternLearningOn = isActive(status, permissions.activity_pattern_learning)
  const crossDeviceSyncOn = isActive(status, permissions.cross_device_sync)
  const memoryGraphOn = isActive(status, permissions.memory_graph_enrichment)
  const mutating = setMutation.isPending || withdrawMutation.isPending

  const statusLabel: Record<ConsentStatus, string> = {
    Valid: t('privacy.consent.status.valid'),
    NotGranted: t('privacy.consent.status.notGranted'),
    Expired: t('privacy.consent.status.expired'),
    UpdateRequired: t('privacy.consent.status.updateRequired'),
  }

  const handleMonitoring = (next: boolean) => {
    // Preserve (spread) the entire current set of permissions and set only the 6 master fields in bulk.
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
    // Preserve (spread) the entire current set of permissions and flip only microphone (to avoid losing other opt-ins).
    setMutation.mutate({ ...permissions, microphone: next })
  }

  const handleFullText = (next: boolean) => {
    setMutation.mutate({ ...permissions, full_text_extraction: next })
  }

  const handleOcrProcessing = (next: boolean) => {
    setMutation.mutate({ ...permissions, ocr_processing: next })
  }

  const handleUnredactedExternalOcr = (next: boolean) => {
    setMutation.mutate({ ...permissions, unredacted_external_ocr: next })
  }

  const handlePatternLearning = (next: boolean) => {
    setMutation.mutate({ ...permissions, activity_pattern_learning: next })
  }

  const handleCrossDeviceSync = (next: boolean) => {
    setMutation.mutate({ ...permissions, cross_device_sync: next })
  }

  const handleMemoryGraph = (next: boolean) => {
    setMutation.mutate({ ...permissions, memory_graph_enrichment: next })
  }

  // #9505 sibling: the inline alert previously rendered the raw Rust wire
  // literal via errorMessageFromInvoke — route both mutations through the
  // shared IPC-error mapping like every other privacy surface (#9492 item 4).
  const lastError = setMutation.isError
    ? describeIpcError(setMutation.error, (key) => t(key), i18n.language)
    : withdrawMutation.isError
      ? describeIpcError(withdrawMutation.error, (key) => t(key), i18n.language)
      : null

  return (
    <Card id="section-consent-toggle" variant="default" padding="lg">
      <CardTitle className="mb-2">{t('privacy.consent.title')}</CardTitle>
      <p className="mb-4 text-content-secondary text-sm">{t('privacy.consent.subtitle')}</p>

      {/* Status line */}
      <p className="mb-4 text-content-strong text-sm" data-testid="consent-status">
        {t('privacy.consent.status.label')}: {statusLabel[status]}
      </p>

      {/* Expired / UpdateRequired → prominent warning (agent fail-closed) */}
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

      {/* IPC error display */}
      {lastError && (
        <Alert variant="error" className="mb-4" title={t('privacy.consent.actionFailed')}>
          {lastError}
        </Alert>
      )}

      <div className="space-y-4">
        {/* Monitoring bundle toggle */}
        <EnumeratedToggle
          testId="consent-monitoring-toggle"
          label={t('privacy.consent.monitoring.label')}
          description={t('privacy.consent.monitoring.description')}
          collectedTitle={t('privacy.consent.monitoring.collectedTitle')}
          collected={[
            t('privacy.consent.monitoring.collected.screenFrames'),
            // #9643 re-review O-2: the bundle grants ocr_processing (M3), so
            // the disclosure under this toggle must name text extraction —
            // same gap I1 closed on the onboarding surface.
            t('privacy.consent.monitoring.collected.ocrText'),
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

        {/* High-sensitivity clipboard / file-access opt-in (off by default) */}
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

        {/* High-sensitivity microphone opt-in (off by default). The disclosure is placed as a
            sibling right below the toggle as a visible warning (role=alert) that *unconditionally*
            announces that, with cloud STT, the raw audio may be sent to a third party, and is
            associated with the microphone toggle via aria-describedby (since a static role="alert"
            is not reliably announced — WAI-ARIA — so it is read along with the toggle). The osNote
            refers to the microphone OS permission, not screen recording. */}
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

        <div className="space-y-2">
          <EnumeratedToggle
            testId="consent-full-text-toggle"
            label={t('privacy.consent.fullText.label')}
            description={t('privacy.consent.fullText.description')}
            collectedTitle={t('privacy.consent.fullText.collectedTitle')}
            collected={[
              t('privacy.consent.fullText.collected.chatText'),
              t('privacy.consent.fullText.collected.activeContext'),
              t('privacy.consent.fullText.collected.providerResult'),
            ]}
            checked={fullTextOn}
            disabled={mutating}
            onChange={handleFullText}
            describedById={FULL_TEXT_DISCLOSURE_ID}
          />
          <Alert id={FULL_TEXT_DISCLOSURE_ID} variant="warning">
            {t('privacy.consent.fullText.disclosure')}
          </Alert>
        </div>

        <div className="space-y-2">
          <EnumeratedToggle
            testId="consent-ocr-processing-toggle"
            label={t('privacy.consent.ocrProcessing.label')}
            description={t('privacy.consent.ocrProcessing.description')}
            collectedTitle={t('privacy.consent.ocrProcessing.collectedTitle')}
            collected={[
              t('privacy.consent.ocrProcessing.collected.screenPixels'),
              t('privacy.consent.ocrProcessing.collected.extractedText'),
              t('privacy.consent.ocrProcessing.collected.retention'),
            ]}
            checked={ocrProcessingOn}
            disabled={mutating}
            onChange={handleOcrProcessing}
            describedById={OCR_PROCESSING_DISCLOSURE_ID}
          />
          <Alert id={OCR_PROCESSING_DISCLOSURE_ID} variant="info">
            {t('privacy.consent.ocrProcessing.disclosure')}
          </Alert>
          {/* #9631: no OCR engine ships on Linux (native_ocr is macOS/Windows
              only) — without this note the consent reads as a working toggle
              that silently does nothing. */}
          {IS_LINUX ? <Alert variant="warning">{t('privacy.consent.ocrProcessing.linuxUnsupported')}</Alert> : null}
        </div>

        <div className="space-y-2">
          <EnumeratedToggle
            testId="consent-unredacted-external-ocr-toggle"
            label={t('privacy.consent.unredactedExternalOcr.label')}
            description={t('privacy.consent.unredactedExternalOcr.description')}
            collectedTitle={t('privacy.consent.unredactedExternalOcr.collectedTitle')}
            collected={[
              t('privacy.consent.unredactedExternalOcr.collected.rawScreenImage'),
              t('privacy.consent.unredactedExternalOcr.collected.windowContext'),
              t('privacy.consent.unredactedExternalOcr.collected.providerResult'),
            ]}
            checked={unredactedExternalOcrOn}
            disabled={mutating}
            onChange={handleUnredactedExternalOcr}
            describedById={UNREDACTED_OCR_DISCLOSURE_ID}
          />
          <Alert id={UNREDACTED_OCR_DISCLOSURE_ID} variant="warning">
            {t('privacy.consent.unredactedExternalOcr.disclosure')}
          </Alert>
        </div>
        {/* #8686 AC2: granular revoke for the three onboarding-granted opt-ins
            that previously only the nuclear withdraw path could remove. */}
        <EnumeratedToggle
          testId="consent-pattern-learning-toggle"
          label={t('privacy.consent.patternLearning.label')}
          description={t('privacy.consent.patternLearning.description')}
          collectedTitle={t('privacy.consent.patternLearning.collectedTitle')}
          collected={[
            t('privacy.consent.patternLearning.collected.habitStats'),
            t('privacy.consent.patternLearning.collected.playbookSignals'),
            t('privacy.consent.patternLearning.collected.coachingTriggers'),
          ]}
          checked={patternLearningOn}
          disabled={mutating}
          onChange={handlePatternLearning}
        />

        <EnumeratedToggle
          testId="consent-cross-device-sync-toggle"
          label={t('privacy.consent.crossDeviceSync.label')}
          description={t('privacy.consent.crossDeviceSync.description')}
          collectedTitle={t('privacy.consent.crossDeviceSync.collectedTitle')}
          collected={[
            t('privacy.consent.crossDeviceSync.collected.syncPayloads'),
            t('privacy.consent.crossDeviceSync.collected.deviceIdentity'),
          ]}
          checked={crossDeviceSyncOn}
          disabled={mutating}
          onChange={handleCrossDeviceSync}
        />

        <EnumeratedToggle
          testId="consent-memory-graph-toggle"
          label={t('privacy.consent.memoryGraphEnrichment.label')}
          description={t('privacy.consent.memoryGraphEnrichment.description')}
          collectedTitle={t('privacy.consent.memoryGraphEnrichment.collectedTitle')}
          collected={[
            t('privacy.consent.memoryGraphEnrichment.collected.memoryEdges'),
            t('privacy.consent.memoryGraphEnrichment.collected.derivedEntities'),
          ]}
          checked={memoryGraphOn}
          disabled={mutating}
          onChange={handleMemoryGraph}
        />
      </div>

      {/* Consent ≠ OS screen-access note (both are required) */}
      <p className="mt-4 text-content-tertiary text-xs">{t('privacy.consent.osNote')}</p>

      {/* Withdrawal + erasure request (visually separated from monitoring OFF) */}
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
        confirmDisabled={withdrawMutation.isPending}
        onConfirm={() => withdrawMutation.mutate()}
        onCancel={() => setShowWithdrawModal(false)}
      />
    </Card>
  )
}
