/**
 * First-run onboarding page — 7-step guide shown before the main shell
 * (Intro → Permissions → Consent → Features → Audio → Coaching → Ready).
 * StepCoaching remains opt-in only at index 5; skipping leaves coaching off.
 */

import {
  Bell,
  BrainCircuit,
  Camera,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  CircleCheckBig,
  Cpu,
  Keyboard,
  Lightbulb,
  Mic,
  Monitor,
  Rocket,
  RotateCcw,
  Search as SearchIcon,
  Shield,
  ShieldCheck,
  Sparkles,
} from 'lucide-react'
import { type ReactNode, useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  type DesktopPermissionSnapshot,
  type DesktopPermissionState,
  fetchDesktopPermissionStatus,
  fetchSettings,
  getConsent,
  requestDesktopNotificationPermission,
  requestDesktopScreenCapturePermission,
  setConsent,
  updateSettings,
} from '../api/client'
import type { ConsentPermissions } from '../api/contracts'
import { errorMessageFromInvoke } from '../api/desktop'
import { Alert, Badge, Button } from '../components/ui'
import { colors, iconSize, motion, radius, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { IS_LINUX, IS_MAC, IS_TAURI, IS_WINDOWS } from '../utils/platform'

interface OnboardingProps {
  onComplete: () => void
}

// Intro → Permissions → Consent → Features → Audio → Coaching → Ready.
// Audio is disclosure-only and default-off: advancing defers microphone capture
// without touching settings, devices, models, or provider endpoints.
const TOTAL_STEPS = 7

// The first-run monitoring consent bundle. Superset of ConsentToggleSection's
// 6-field MONITORING_FIELDS by `ocr_processing` (#9631): without OCR consent,
// captured frames carry no text, so /search finds nothing and scene analysis
// is empty — the user reads that as "search is broken," not "a consent is
// off." OCR stays individually revocable in Settings → Privacy. Every other
// high-sensitivity / additional opt-in field remains false.
const MONITORING_FIELDS = [
  'screen_capture',
  'ocr_processing',
  'window_title_collection',
  'app_usage_analytics',
  'process_monitoring',
  'input_activity',
  'telemetry',
] as const satisfies readonly (keyof ConsentPermissions)[]

/** Builds a fresh grant permission set that starts with every permission false and turns on only the monitoring bundle. */
function buildMonitoringGrant(): ConsentPermissions {
  const permissions: ConsentPermissions = {
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
    unredacted_external_ocr: false,
    memory_graph_retrieval_ranking: false,
    memory_vault_mirror: false,
  }
  for (const field of MONITORING_FIELDS) {
    permissions[field] = true
  }
  return permissions
}

async function invokeCommand(cmd: string) {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke(cmd)
  } catch {
    // Standalone / dev mode — no Tauri runtime
  }
}

async function openPermissionSettings(
  permissionKind: 'accessibility' | 'screen_capture' | 'microphone' | 'input_monitoring',
) {
  const { invoke } = await import('@tauri-apps/api/core')
  await invoke('open_desktop_permission_settings', { permissionKind })
}

/* ── Step content components ── */

function StepIntro() {
  const { t } = useTranslation()
  return (
    <div className="flex flex-col items-center text-center">
      <div className={cn('mb-6 flex items-center justify-center rounded-full bg-brand-signal/15 p-4', motion.opacity)}>
        <Rocket className={cn(iconSize.hero, 'text-brand-text')} />
      </div>
      <h2 className={cn(typography.h1, colors.text.primary, 'mb-3')}>{t('onboarding.step1Title')}</h2>
      <p className={cn(typography.body, colors.text.secondary, 'max-w-sm')}>{t('onboarding.step1Desc')}</p>
    </div>
  )
}

function PermissionRow({ icon, label }: { icon: ReactNode; label: string }) {
  return (
    <div className={cn('flex items-center gap-3 rounded-lg bg-surface-muted px-4 py-3', motion.colors)}>
      <div className="flex items-center justify-center rounded-md bg-brand-signal/15 p-2">{icon}</div>
      <span className={cn(typography.body, typography.weight.medium, colors.text.primary)}>{label}</span>
    </div>
  )
}

function badgeColorForPermission(state: DesktopPermissionState): 'success' | 'warning' | 'default' {
  switch (state) {
    case 'granted':
      return 'success'
    case 'needs_attention':
      return 'warning'
    case 'not_required':
    case 'unavailable':
      return 'default'
  }
}

interface OnboardingPermissionRowAction {
  id: 'accessibility' | 'screen_capture' | 'microphone' | 'input_monitoring' | 'notifications'
  label: string
  onClick: () => void
}

interface OnboardingPermissionRow {
  id: string
  icon: ReactNode
  label: string
  description: string
  state: DesktopPermissionState
  action?: OnboardingPermissionRowAction
}

function PermissionStatusPill({
  state,
  loading = false,
  label,
}: {
  state: DesktopPermissionState
  loading?: boolean
  label: string
}) {
  if (state === 'granted') {
    return (
      <div
        className={cn(
          'flex items-center gap-2 rounded-lg bg-semantic-success/10 px-3 py-2 text-semantic-success',
          typography.body,
          typography.weight.medium,
        )}
      >
        <CircleCheckBig className={iconSize.base} />
        <span>{label}</span>
      </div>
    )
  }

  return <Badge color={badgeColorForPermission(state)}>{loading ? '...' : label}</Badge>
}

function OnboardingPermissionCard({
  row,
  actionLoading,
  stateLabel,
}: {
  row: OnboardingPermissionRow
  actionLoading: boolean
  stateLabel: string
}) {
  return (
    <div className="rounded-xl border border-DEFAULT bg-surface-muted/70 p-4 text-left">
      <div className="flex flex-col gap-4 md:flex-row md:items-start md:justify-between">
        <div className="min-w-0 space-y-2">
          <div className="flex items-center gap-3">
            <div className="flex items-center justify-center rounded-md bg-brand-signal/15 p-2">{row.icon}</div>
            <h3 className={cn(typography.body, typography.weight.medium, colors.text.primary)}>{row.label}</h3>
          </div>
          <p className={cn(typography.small, colors.text.secondary, 'max-w-md')}>{row.description}</p>
        </div>

        <div className="flex min-w-[220px] flex-col items-stretch gap-2 md:items-end">
          <PermissionStatusPill state={row.state} loading={actionLoading} label={stateLabel} />
          {row.action && row.state !== 'granted' && (
            <Button
              type="button"
              variant="secondary"
              size="sm"
              isLoading={actionLoading}
              onClick={row.action.onClick}
              className="w-full md:w-auto"
            >
              {row.action.label}
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}

function StepPermissions({ onReadyChange }: { onReadyChange: (ready: boolean) => void }) {
  const { t } = useTranslation()
  const [permissionStatus, setPermissionStatus] = useState<DesktopPermissionSnapshot | null>(null)
  const [loading, setLoading] = useState(IS_TAURI)
  const [error, setError] = useState<string | null>(null)
  const [screenCaptureRestartHint, setScreenCaptureRestartHint] = useState(false)
  const [actionLoading, setActionLoading] = useState<
    'accessibility' | 'screen_capture' | 'microphone' | 'input_monitoring' | 'notifications' | 'refresh' | null
  >(null)
  const iconCls = cn(iconSize.base, 'text-brand-text')

  const loadPermissionStatus = useCallback(async () => {
    if (!IS_TAURI) {
      setLoading(false)
      setPermissionStatus(null)
      setError(null)
      onReadyChange(true)
      return
    }

    setLoading(true)
    setError(null)
    try {
      const snapshot = await fetchDesktopPermissionStatus()
      setPermissionStatus(snapshot)
    } catch (nextError) {
      const message = nextError instanceof Error ? nextError.message : t('onboarding.step2PermissionCheckFailed')
      setError(message)
    } finally {
      setLoading(false)
    }
  }, [onReadyChange, t])

  useEffect(() => {
    void loadPermissionStatus()
  }, [loadPermissionStatus])

  useEffect(() => {
    if (!IS_TAURI) return
    const refreshOnFocus = () => void loadPermissionStatus()
    window.addEventListener('focus', refreshOnFocus)
    return () => window.removeEventListener('focus', refreshOnFocus)
  }, [loadPermissionStatus])

  const requiredReady = useMemo(() => {
    if (!IS_TAURI || !IS_MAC) return true
    if (!permissionStatus) return false
    return permissionStatus.accessibility.state === 'granted' && permissionStatus.screen_capture.state === 'granted'
  }, [permissionStatus])

  useEffect(() => {
    onReadyChange(requiredReady)
  }, [onReadyChange, requiredReady])

  const handleOpenPermissionSettings = useCallback(
    async (kind: 'accessibility' | 'screen_capture' | 'microphone' | 'input_monitoring') => {
      setActionLoading(kind)
      setError(null)
      try {
        await openPermissionSettings(kind)
      } catch (nextError) {
        const message = nextError instanceof Error ? nextError.message : t('onboarding.step2SystemSettingsOpenFailed')
        setError(message)
      } finally {
        setActionLoading(null)
      }
    },
    [t],
  )

  const handleRequestScreenCapturePermission = useCallback(async () => {
    setActionLoading('screen_capture')
    setError(null)
    try {
      const snapshot = await requestDesktopScreenCapturePermission()
      setPermissionStatus(snapshot)
      setScreenCaptureRestartHint(snapshot.screen_capture.state === 'granted')
      if (snapshot.screen_capture.state !== 'granted') {
        await openPermissionSettings('screen_capture')
      }
    } catch (nextError) {
      const message = nextError instanceof Error ? nextError.message : t('onboarding.step2SystemSettingsOpenFailed')
      setError(message)
    } finally {
      setActionLoading(null)
    }
  }, [t])

  const handleRequestNotificationPermission = useCallback(async () => {
    setActionLoading('notifications')
    setError(null)
    try {
      const snapshot = await requestDesktopNotificationPermission()
      setPermissionStatus(snapshot)
    } catch (nextError) {
      const message = nextError instanceof Error ? nextError.message : t('onboarding.step2NotificationRequestFailed')
      setError(message)
    } finally {
      setActionLoading(null)
    }
  }, [t])

  const permissionRows: OnboardingPermissionRow[] = IS_MAC
    ? [
        {
          id: 'accessibility',
          icon: <Monitor className={iconCls} />,
          label: t('onboarding.step2Accessibility'),
          description: t('settings.permissionAccessibilityDesc'),
          state: permissionStatus?.accessibility.state ?? 'needs_attention',
          action: {
            id: 'accessibility',
            label: t('onboarding.step2AccessibilityAction'),
            onClick: () => void handleOpenPermissionSettings('accessibility'),
          },
        },
        {
          id: 'screen-capture',
          icon: <Camera className={iconCls} />,
          label: t('onboarding.step2ScreenCapture'),
          description: t('settings.permissionScreenCaptureDesc'),
          state: permissionStatus?.screen_capture.state ?? 'needs_attention',
          action: {
            id: 'screen_capture',
            label: t('onboarding.step2ScreenCaptureAction'),
            onClick: () => void handleRequestScreenCapturePermission(),
          },
        },
        {
          id: 'notifications',
          icon: <Bell className={iconCls} />,
          label: t('onboarding.step2Notifications'),
          description: t('settings.permissionNotificationsDesc'),
          state: permissionStatus?.notifications.state ?? 'needs_attention',
          action:
            permissionStatus?.notifications.status_reason === 'macos_notifications_not_determined'
              ? {
                  id: 'notifications',
                  label: t('settings.permissionMacNotificationsRequestAction'),
                  onClick: () => void handleRequestNotificationPermission(),
                }
              : undefined,
        },
        {
          id: 'microphone',
          icon: <Mic className={iconCls} />,
          label: t('settings.permissionMicrophoneLabel'),
          description: t('settings.permissionMicrophoneDesc'),
          state: permissionStatus?.microphone?.state ?? 'needs_attention',
          action: {
            id: 'microphone',
            label: t('settings.permissionOpenSystemSettingsAction'),
            onClick: () => void handleOpenPermissionSettings('microphone'),
          },
        },
        {
          id: 'input-monitoring',
          icon: <Keyboard className={iconCls} />,
          label: t('settings.permissionInputMonitoringLabel'),
          description: t('settings.permissionInputMonitoringDesc'),
          state: permissionStatus?.input_monitoring?.state ?? 'needs_attention',
          action: {
            id: 'input_monitoring',
            label: t('settings.permissionOpenSystemSettingsAction'),
            onClick: () => void handleOpenPermissionSettings('input_monitoring'),
          },
        },
      ]
    : IS_WINDOWS
      ? [
          {
            id: 'windows-accessibility',
            icon: <Monitor className={iconCls} />,
            label: t('onboarding.step2WindowsAccessibility'),
            description: t('settings.permissionWindowsAccessibilityDesc'),
            state: permissionStatus?.accessibility.state ?? 'not_required',
          },
          {
            id: 'windows-screen-capture',
            icon: <Camera className={iconCls} />,
            label: t('onboarding.step2WindowsScreenCapture'),
            description: t('settings.permissionWindowsScreenCaptureDesc'),
            state: permissionStatus?.screen_capture.state ?? 'not_required',
          },
          {
            id: 'windows-notifications',
            icon: <Bell className={iconCls} />,
            label: t('onboarding.step2Notifications'),
            description: t('settings.permissionWindowsNotificationsDesc'),
            state: permissionStatus?.notifications.state ?? 'not_required',
          },
        ]
      : IS_LINUX
        ? [
            {
              id: 'linux-accessibility',
              icon: <Monitor className={iconCls} />,
              label: t('onboarding.step2LinuxAccessibility'),
              description: t('settings.permissionLinuxAccessibilityDesc'),
              state: permissionStatus?.accessibility.state ?? 'unavailable',
            },
            {
              id: 'linux-screen-capture',
              icon: <Camera className={iconCls} />,
              label: t('onboarding.step2LinuxScreenCapture'),
              description: t('settings.permissionLinuxScreenCaptureDesc'),
              state: permissionStatus?.screen_capture.state ?? 'unavailable',
            },
            {
              id: 'linux-notifications',
              icon: <Bell className={iconCls} />,
              label: t('onboarding.step2Notifications'),
              description: t('settings.permissionLinuxNotificationsDesc'),
              state: permissionStatus?.notifications.state ?? 'not_required',
            },
          ]
        : []

  const description = IS_MAC
    ? t(
        'onboarding.step2DescMac',
        'macOS requires Accessibility, Screen Recording, and notification access for the full Maekon experience.',
      )
    : IS_WINDOWS
      ? t(
          'onboarding.step2DescWindows',
          'Windows usually does not require separate permission prompts, but Maekon still depends on desktop access, screen capture, and notification delivery being available.',
        )
      : IS_LINUX
        ? t(
            'onboarding.step2DescLinux',
            'Linux usually relies on session services instead of OS prompts. Confirm accessibility, screen capture, and notifications are available in your desktop session.',
          )
        : t('onboarding.step2Desc')

  return (
    <div className="flex flex-col items-center text-center">
      <div className={cn('mb-6 flex items-center justify-center rounded-full bg-brand-signal/15 p-4', motion.opacity)}>
        <Shield className={cn(iconSize.hero, 'text-brand-text')} />
      </div>
      <h2 className={cn(typography.h1, colors.text.primary, 'mb-3')}>{t('onboarding.step2Title')}</h2>
      <p className={cn(typography.body, colors.text.secondary, 'mb-6 max-w-sm')}>{description}</p>

      <div className="mb-4 flex w-full max-w-2xl justify-end">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          isLoading={loading || actionLoading === 'refresh'}
          onClick={() => {
            setActionLoading('refresh')
            void loadPermissionStatus().finally(() => setActionLoading(null))
          }}
        >
          <RotateCcw className={cn(iconSize.base, 'mr-2')} />
          {t('settings.permissionRefreshAction')}
        </Button>
      </div>

      {error && (
        <div className="mb-4 w-full max-w-2xl">
          <Alert variant="error" title={t('settings.permissionCheckFailedTitle')}>
            <p>{error}</p>
          </Alert>
        </div>
      )}

      <div className="flex w-full max-w-2xl flex-col gap-3">
        {permissionRows.map((row) => (
          <OnboardingPermissionCard
            key={row.id}
            row={row}
            actionLoading={actionLoading === row.action?.id}
            stateLabel={
              row.state === 'granted'
                ? t('onboarding.step2PermissionEnabled')
                : row.state === 'needs_attention'
                  ? t('settings.permissionStateNeedsAttention')
                  : row.state === 'not_required'
                    ? t('settings.permissionStateNotRequired')
                    : t('settings.permissionStateUnavailable')
            }
          />
        ))}
      </div>

      {IS_MAC && (
        <div className="mt-4 w-full max-w-2xl">
          {requiredReady ? (
            <Alert variant="success" title={t('onboarding.step2ReadyTitle')}>
              <p>{t('onboarding.step2ReadyDesc')}</p>
            </Alert>
          ) : (
            <Alert variant="warning" title={t('onboarding.step2RequiredTitle')}>
              <p>{t('onboarding.step2RequiredDesc')}</p>
            </Alert>
          )}
        </div>
      )}

      {IS_MAC && screenCaptureRestartHint && permissionStatus?.screen_capture.state === 'granted' && (
        <div className="mt-4 w-full max-w-2xl">
          <Alert variant="info" title={t('onboarding.step2ScreenCaptureRestartTitle')}>
            <p>{t('onboarding.step2ScreenCaptureRestartDesc')}</p>
          </Alert>
        </div>
      )}

      {!IS_MAC && permissionRows.length > 0 && (
        <div className="mt-4 w-full max-w-2xl">
          <Alert variant="info" title={t('settings.permissionManualCheckTitle')}>
            <p>{t('settings.permissionManualCheckDesc')}</p>
          </Alert>
        </div>
      )}

      {permissionRows.length === 0 && !loading && (
        <div className="w-full max-w-xs">
          <PermissionRow icon={<Shield className={iconCls} />} label={t('common.loading')} />
        </div>
      )}
      {loading && (
        <div className="mt-4 flex items-center gap-2 text-content-secondary text-sm">
          <RotateCcw className={cn(iconSize.base, 'animate-spin')} />
          <span>{t('settings.permissionChecking')}</span>
        </div>
      )}
      {!requiredReady && IS_MAC && (
        <p className="mt-4 text-content-secondary text-sm">{t('onboarding.step2RequiredContinueHint')}</p>
      )}
      {!loading && permissionRows.length === 0 && (
        <div className="mt-4 flex items-center gap-2 text-semantic-warning text-sm">
          <CircleAlert className={iconSize.base} />
          <span>{t('settings.permissionManualCheckDesc')}</span>
        </div>
      )}
      {!loading && permissionRows.length > 0 && requiredReady && IS_MAC && (
        <div className="mt-4 flex items-center gap-2 text-semantic-success text-sm">
          <CircleCheckBig className={iconSize.base} />
          <span>{t('onboarding.step2ReadyInline')}</span>
        </div>
      )}
    </div>
  )
}

/* ── Consent step (#4629 A.2 task 4) ── */

// Enumerates every item collected when monitoring is ON (reuses privacy.consent.monitoring.collected.*).
// So that onboarding completion provisions both OS permissions and GDPR consent, grant only via an explicit click after the informational disclosure.
const CONSENT_COLLECTED_KEYS = [
  'screenFrames',
  // #9643 review I1: the grant bundle includes ocr_processing (#9631), so the
  // disclosure list shown ABOVE the grant button must name text extraction.
  'ocrText',
  'windowTitles',
  'appUsage',
  'inputActivity',
  'activeProcesses',
  'systemMetrics',
  'activitySegments',
  'focusMetrics',
  'digests',
  'memoryGraph',
  'embeddings',
  'guiInteractions',
] as const

function StepConsent() {
  const { t } = useTranslation()
  const [granting, setGranting] = useState(false)
  const [granted, setGranted] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Explicit, informed grant: call set_consent only on click (no auto-grant).
  const handleGrant = useCallback(async () => {
    setGranting(true)
    setError(null)
    try {
      await setConsent(buildMonitoringGrant())
      setGranted(true)
    } catch (nextError) {
      setError(errorMessageFromInvoke(nextError))
    } finally {
      setGranting(false)
    }
  }, [])

  return (
    <div className="flex flex-col items-center text-center">
      <div className={cn('mb-6 flex items-center justify-center rounded-full bg-brand-signal/15 p-4', motion.opacity)}>
        <ShieldCheck className={cn(iconSize.hero, 'text-brand-text')} />
      </div>
      <h2 className={cn(typography.h1, colors.text.primary, 'mb-3')}>{t('onboarding.consent.title')}</h2>
      <p className={cn(typography.body, colors.text.secondary, 'mb-6 max-w-sm')}>
        {t('onboarding.consent.description')}
      </p>

      {/* Before granting: enumerate all collected items (informational). After granting: replace with the confirmed state. */}
      {granted ? (
        <div className="w-full max-w-md" data-testid="onboarding-consent-granted">
          <Alert variant="success" title={t('onboarding.consent.granted')}>
            <p>{t('onboarding.consent.grantedNote')}</p>
          </Alert>
        </div>
      ) : (
        <div className="w-full max-w-md text-left">
          <p className={cn(typography.small, colors.text.secondary, 'mb-2')}>
            {t('onboarding.consent.collectedTitle')}
          </p>
          <ul className="mb-4 grid list-inside list-disc grid-cols-1 gap-x-4 text-content-tertiary text-xs sm:grid-cols-2">
            {CONSENT_COLLECTED_KEYS.map((key) => (
              <li key={key}>{t(`privacy.consent.monitoring.collected.${key}`)}</li>
            ))}
          </ul>

          {error && (
            <Alert variant="error" className="mb-4" title={t('onboarding.consent.error', { error })}>
              <p>{error}</p>
            </Alert>
          )}

          <Button
            type="button"
            variant="primary"
            size="md"
            className="w-full"
            isLoading={granting}
            onClick={() => void handleGrant()}
            data-testid="onboarding-consent-grant"
          >
            <ShieldCheck className={cn(iconSize.base, 'mr-2')} />
            {granting ? t('onboarding.consent.granting') : t('onboarding.consent.grantButton')}
          </Button>

          <p className="mt-4 text-content-tertiary text-xs">{t('onboarding.consent.osNote')}</p>
        </div>
      )}
    </div>
  )
}

function FeatureItem({ icon, label }: { icon: ReactNode; label: string }) {
  return (
    <div className={cn('flex items-center gap-3 rounded-lg bg-surface-muted px-4 py-3', motion.colors)}>
      <div className="flex items-center justify-center rounded-md bg-brand-signal/15 p-2">{icon}</div>
      <span className={cn(typography.body, typography.weight.medium, colors.text.primary)}>{label}</span>
    </div>
  )
}

/**
 * StepFeatures — index 3 of 6. Enumerates what Maekon can do (Capture /
 * Analysis / Suggestions / Search) and, per #8059 G2b, offers a low-friction,
 * privacy-first opt-in that pre-enables the built-but-hidden AI features
 * (local embedding + AI semantic search + AI daily-digest narrative).
 *
 * The opt-in mirrors the StepCoaching explicit-click pattern (#5707): the user
 * must click "Enable AI features" to activate; skipping the step leaves the
 * three analysis flags at their privacy-first default (off). Everything runs
 * on-device and takes effect after the app restarts.
 */
function StepFeatures() {
  const { t } = useTranslation()
  const iconCls = cn(iconSize.base, 'text-brand-text')
  const [enabling, setEnabling] = useState(false)
  const [enabled, setEnabled] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Explicit, informed opt-in: fetch current settings, then flip the three
  // analysis flags (analysis + embedding + llm_summary) that make up the AI
  // features bundle. No auto-enable — only on click.
  const handleEnableAiFeatures = useCallback(async () => {
    setEnabling(true)
    setError(null)
    try {
      const current = await fetchSettings()
      await updateSettings({
        ...current,
        analysis: {
          ...current.analysis,
          enabled: true,
          embedding_enabled: true,
          llm_summary_enabled: true,
        },
      })
      setEnabled(true)
    } catch (nextError) {
      setError(errorMessageFromInvoke(nextError))
    } finally {
      setEnabling(false)
    }
  }, [])

  return (
    <div className="flex flex-col items-center text-center">
      <div className={cn('mb-6 flex items-center justify-center rounded-full bg-brand-signal/15 p-4', motion.opacity)}>
        <Cpu className={cn(iconSize.hero, 'text-brand-text')} />
      </div>
      <h2 className={cn(typography.h1, colors.text.primary, 'mb-3')}>{t('onboarding.step3Title')}</h2>
      <p className={cn(typography.body, colors.text.secondary, 'mb-6 max-w-sm')}>{t('onboarding.step3Desc')}</p>
      <div className="flex w-full max-w-xs flex-col gap-3">
        <FeatureItem icon={<Camera className={iconCls} />} label={t('onboarding.step3Capture')} />
        <FeatureItem icon={<Cpu className={iconCls} />} label={t('onboarding.step3Analysis')} />
        <FeatureItem icon={<Lightbulb className={iconCls} />} label={t('onboarding.step3Suggestions')} />
        <FeatureItem icon={<SearchIcon className={iconCls} />} label={t('onboarding.step3Search')} />
      </div>

      {/* #8059 G2b: privacy-first AI-features opt-in (default off). */}
      <div className="mt-6 w-full max-w-md text-left">
        {enabled ? (
          <div data-testid="onboarding-aifeatures-enabled">
            <Alert variant="success" title={t('onboarding.aiFeatures.enabled')}>
              <p>{t('onboarding.aiFeatures.enabledNote')}</p>
            </Alert>
          </div>
        ) : (
          <>
            <p className={cn(typography.small, colors.text.secondary, 'mb-3')}>
              {t('onboarding.aiFeatures.optInHint')}
            </p>

            {error && (
              <Alert variant="error" className="mb-4" title={t('onboarding.aiFeatures.error', { error })}>
                <p>{error}</p>
              </Alert>
            )}

            <Button
              type="button"
              variant="secondary"
              size="md"
              className="w-full"
              isLoading={enabling}
              onClick={() => void handleEnableAiFeatures()}
              data-testid="onboarding-aifeatures-enable"
            >
              <Sparkles className={cn(iconCls, 'mr-2')} />
              {enabling ? t('onboarding.aiFeatures.enabling') : t('onboarding.aiFeatures.enableButton')}
            </Button>
          </>
        )}
      </div>
    </div>
  )
}

/**
 * StepCoaching — #5707 Tier-4 opt-in step (index 4 of 6).
 *
 * Mirrors the StepConsent explicit-click pattern (#4629): user must click
 * "Enable coaching" to activate. Skipping the step leaves coaching.enabled=false.
 * On enable: merge-grants Tier-4 consent (activity_pattern_learning) +
 * sets coaching.enabled=true via the REST settings API.
 */
function StepCoaching() {
  const { t } = useTranslation()
  const [enabling, setEnabling] = useState(false)
  const [enabled, setEnabled] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const ENABLES_LIST_KEYS = ['item1', 'item2', 'item3', 'item4'] as const

  const iconCls = cn(iconSize.base, 'text-brand-text')

  // Merge-grant Tier-4 consent (activity_pattern_learning) + set coaching.enabled=true.
  // Fetches current consent first so only activity_pattern_learning is added;
  // other consent fields are preserved (#5707).
  const handleEnable = useCallback(async () => {
    setEnabling(true)
    setError(null)
    try {
      // Fetch current consent snapshot, merge Tier-4 field, then re-grant.
      const snapshot = await getConsent()
      await setConsent({ ...snapshot.permissions, activity_pattern_learning: true })
      // Set coaching.enabled=true via REST settings write.
      const current = await fetchSettings()
      await updateSettings({ ...current, coaching: { ...current.coaching, enabled: true } })
      setEnabled(true)
    } catch (nextError) {
      setError(errorMessageFromInvoke(nextError))
    } finally {
      setEnabling(false)
    }
  }, [])

  return (
    <div className="flex flex-col items-center text-center">
      <div className={cn('mb-6 flex items-center justify-center rounded-full bg-brand-signal/15 p-4', motion.opacity)}>
        <BrainCircuit className={cn(iconSize.hero, 'text-brand-text')} />
      </div>
      <h2 className={cn(typography.h1, colors.text.primary, 'mb-3')}>{t('onboarding.coaching.title')}</h2>
      <p className={cn(typography.body, colors.text.secondary, 'mb-6 max-w-sm')}>
        {t('onboarding.coaching.description')}
      </p>

      {enabled ? (
        <div className="w-full max-w-md" data-testid="onboarding-coaching-enabled">
          <Alert variant="success" title={t('onboarding.coaching.enabled')}>
            <p>{t('onboarding.coaching.activeNote')}</p>
          </Alert>
        </div>
      ) : (
        <div className="w-full max-w-md text-left">
          <ul className="mb-4 grid list-inside list-disc grid-cols-1 gap-x-4 text-content-tertiary text-xs sm:grid-cols-2">
            {ENABLES_LIST_KEYS.map((key) => (
              <li key={key}>{t(`onboarding.coaching.enablesList.${key}`)}</li>
            ))}
          </ul>

          {error && (
            <Alert variant="error" className="mb-4" title={t('onboarding.coaching.error', { error })}>
              <p>{error}</p>
            </Alert>
          )}

          <Button
            type="button"
            variant="primary"
            size="md"
            className="w-full"
            isLoading={enabling}
            onClick={() => void handleEnable()}
            data-testid="onboarding-coaching-enable"
          >
            <BrainCircuit className={cn(iconCls, 'mr-2')} />
            {enabling ? t('onboarding.coaching.enabling') : t('onboarding.coaching.enableButton')}
          </Button>

          <p className="mt-4 text-content-tertiary text-xs">{t('onboarding.coaching.activeNote')}</p>
        </div>
      )}
    </div>
  )
}

function StepAudioDeferred() {
  const { t } = useTranslation()
  return (
    <div className="flex flex-col items-center text-center" data-testid="onboarding-audio-deferred">
      <div className={cn('mb-6 flex items-center justify-center rounded-full bg-brand-signal/15 p-4', motion.opacity)}>
        <Mic className={cn(iconSize.hero, 'text-brand-text')} />
      </div>
      <h2 className={cn(typography.h1, colors.text.primary, 'mb-3')}>{t('onboarding.audio.title')}</h2>
      <p className={cn(typography.body, colors.text.secondary, 'mb-6 max-w-md')}>{t('onboarding.audio.description')}</p>
      <div className="w-full max-w-md text-left">
        <Alert variant="info" title={t('onboarding.audio.statusTitle')}>
          <p>{t('onboarding.audio.statusDescription')}</p>
        </Alert>
        <ul className="mt-4 space-y-2 text-content-secondary text-sm">
          <li>{t('onboarding.audio.localNote')}</li>
          <li>{t('onboarding.audio.egressNote')}</li>
        </ul>
      </div>
    </div>
  )
}

function StepReady() {
  const { t } = useTranslation()
  const shortcut = IS_MAC ? '⌘K' : 'Ctrl+K'
  // #9631: the recovery card promised above surfaces in the suggestions
  // panel (overlay webview), which has no main-window route — without this
  // disclosure a user who closes the tracking panel can never find it.
  // #9643 review I6: the registrar may have fallen back to an alternate chord
  // (Windows screenshot utilities intercept the primary, #4698), so ask the
  // shortcut registry for the EFFECTIVE accelerator instead of hardcoding —
  // the static value stays as the non-Tauri/standalone fallback.
  const [suggestionsShortcut, setSuggestionsShortcut] = useState(IS_MAC ? '⌘⇧S' : 'Ctrl+Shift+S')
  useEffect(() => {
    let cancelled = false
    ;(async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        const status = await invoke<{
          records: Array<{
            id: string
            primary_accelerator: string
            fallback_accelerator: string | null
            fallback_status: string | null
          }>
        }>('get_global_shortcut_status')
        const record = status.records.find((r) => r.id === 'suggestions-toggle')
        if (!record || cancelled) return
        const effective =
          record.fallback_status === 'registered' && record.fallback_accelerator
            ? record.fallback_accelerator
            : record.primary_accelerator
        setSuggestionsShortcut(
          effective
            .replace('CmdOrCtrl', IS_MAC ? '⌘' : 'Ctrl')
            .replace('+Shift+', IS_MAC ? '⇧' : '+Shift+')
            // #9643 re-review M2: the Windows-reason fallback chord renders
            // correctly on macOS too (⌥, no stray '+').
            .replace('+Alt+', IS_MAC ? '⌥' : '+Alt+'),
        )
      } catch {
        // Standalone / dev mode — keep the platform default.
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])
  // #9811: the consent step (step 2) does not gate the Next button —
  // consent must stay
  // freely refusable — so a user can reach this screen without granting. When
  // that happens the app collects nothing (`get_capture_status` reports
  // `consent_granted:false, permitted:false`) and the timeline is empty
  // forever. Declaring "everything is set up" there is simply false, and it is
  // the reason a new install looks broken rather than switched off.
  //
  // Consent is NOT forced here. The claim is corrected and the grant is offered
  // in place, so refusing stays a real option that is honestly described.
  const [consentGranted, setConsentGranted] = useState<boolean | null>(null)
  const [granting, setGranting] = useState(false)
  const refreshConsent = useCallback(async () => {
    try {
      const snapshot = await getConsent()
      setConsentGranted(snapshot.status === 'Valid' && snapshot.permissions.screen_capture)
    } catch {
      // Standalone / non-Tauri preview has no consent backend to report on.
      // Treat as "nothing to warn about" rather than accusing the user.
      setConsentGranted(true)
    }
  }, [])
  useEffect(() => {
    void refreshConsent()
  }, [refreshConsent])

  const grantHere = useCallback(async () => {
    setGranting(true)
    try {
      await setConsent(buildMonitoringGrant())
      await refreshConsent()
    } finally {
      setGranting(false)
    }
  }, [refreshConsent])

  const collectionOff = consentGranted === false

  return (
    <div className="flex flex-col items-center text-center">
      <div className={cn('mb-6 flex items-center justify-center rounded-full bg-brand-signal/15 p-4', motion.opacity)}>
        <Rocket className={cn(iconSize.hero, 'text-brand-text')} />
      </div>
      <h2 className={cn(typography.h1, colors.text.primary, 'mb-3')}>
        {collectionOff
          ? t('onboarding.readyCollectionOffTitle', 'Starting with collection off')
          : t('onboarding.step4Title')}
      </h2>
      <p className={cn(typography.body, colors.text.secondary, 'mb-4 max-w-sm')}>
        {collectionOff
          ? t(
              'onboarding.readyCollectionOffDesc',
              'You skipped consent, so Maekon is not collecting anything. Your timeline and suggestions will stay empty. Turn it on now, or later in Settings > Privacy.',
            )
          : t('onboarding.step4Desc')}
      </p>
      {collectionOff && (
        <div className="mb-4 w-full max-w-sm" data-testid="onboarding-ready-collection-off">
          <Button type="button" variant="primary" size="md" className="w-full" isLoading={granting} onClick={grantHere}>
            {t('onboarding.readyGrantNow', 'Turn on collection now')}
          </Button>
        </div>
      )}
      {/* #8686 AC5: set the first-run expectation — the first source-backed
          recovery card surfaces in the tracking panel within ~15 minutes of
          normal work, so a new user knows where to look and when. */}
      <p className={cn(typography.body, colors.text.secondary, 'mb-4 max-w-sm')}>
        {t('onboarding.step4RecoveryCardHint')}
      </p>
      <div
        role="note"
        className={cn(
          'mb-3 flex items-start gap-2 rounded-lg bg-surface-muted px-4 py-3 text-left',
          'max-w-sm',
          motion.colors,
        )}
      >
        <Lightbulb className={cn(iconSize.sm, 'mt-0.5 flex-shrink-0 text-brand-text')} aria-hidden="true" />
        <p className={cn(typography.small, colors.text.secondary)}>
          {t('onboarding.step4SuggestionsShortcut', { shortcut: suggestionsShortcut })}
        </p>
      </div>
      <div
        role="note"
        className={cn(
          'flex items-start gap-2 rounded-lg bg-surface-muted px-4 py-3 text-left',
          'max-w-sm',
          motion.colors,
        )}
      >
        <Lightbulb className={cn(iconSize.sm, 'mt-0.5 flex-shrink-0 text-brand-text')} aria-hidden="true" />
        <p className={cn(typography.small, colors.text.secondary)}>{t('onboarding.step4Tip', { shortcut })}</p>
      </div>
    </div>
  )
}

/* ── Step indicator dots ── */

function StepDots({ current, total }: { current: number; total: number }) {
  const { t } = useTranslation()
  const steps = Array.from({ length: total }, (_, i) => ({ id: `step-${i + 1}`, index: i }))
  return (
    <fieldset className="m-0 flex items-center gap-2 border-none p-0" aria-label={t('onboarding.stepIndicatorLabel')}>
      {steps.map((step) => (
        <div
          key={step.id}
          className={cn(
            'h-2 rounded-full',
            motion.all,
            step.index === current ? 'w-6 bg-brand-signal' : 'w-2 bg-surface-muted',
          )}
          aria-current={step.index === current ? 'step' : undefined}
        />
      ))}
    </fieldset>
  )
}

/* ── Main onboarding component ── */

export default function Onboarding({ onComplete }: OnboardingProps) {
  const { t } = useTranslation()
  const [step, setStep] = useState(0)
  const [permissionStepReady, setPermissionStepReady] = useState(!IS_TAURI || !IS_MAC)

  const handleComplete = useCallback(async () => {
    await invokeCommand('complete_onboarding')
    onComplete()
  }, [onComplete])

  const handleSkip = useCallback(async () => {
    await invokeCommand('complete_onboarding')
    onComplete()
  }, [onComplete])

  const isFirst = step === 0
  const isLast = step === TOTAL_STEPS - 1
  const isPermissionStep = step === 1
  const canAdvance = !isPermissionStep || permissionStepReady

  return (
    <div className="flex min-h-screen items-center justify-center bg-surface-sunken p-4">
      <div className={cn('flex w-full max-w-lg flex-col items-center', radius.lg, 'bg-surface-elevated p-8 shadow-xl')}>
        {/* Step dots */}
        <div className="mb-8">
          <StepDots current={step} total={TOTAL_STEPS} />
        </div>

        {/* Step content */}
        <div className="mb-8 w-full">
          {step === 0 && <StepIntro />}
          {step === 1 && <StepPermissions onReadyChange={setPermissionStepReady} />}
          {step === 2 && <StepConsent />}
          {step === 3 && <StepFeatures />}
          {step === 4 && <StepAudioDeferred />}
          {/* #5707: StepCoaching remains opt-in only; skipping leaves default-off intact */}
          {step === 5 && <StepCoaching />}
          {step === 6 && <StepReady />}
        </div>

        {/* Navigation buttons */}
        <div className="flex w-full items-center justify-between">
          <div>
            {!isFirst && (
              <Button variant="ghost" size="md" onClick={() => setStep((s) => s - 1)}>
                <ChevronLeft className={cn(iconSize.base, 'mr-1')} />
                {t('onboarding.back')}
              </Button>
            )}
          </div>

          <div className="flex items-center gap-3">
            <Button variant="ghost" size="md" data-testid="onboarding-skip" onClick={handleSkip}>
              {t('onboarding.skip')}
            </Button>

            {isLast ? (
              <Button variant="primary" size="md" onClick={handleComplete}>
                {t('onboarding.complete')}
              </Button>
            ) : (
              <Button variant="primary" size="md" disabled={!canAdvance} onClick={() => setStep((s) => s + 1)}>
                {t('onboarding.next')}
                <ChevronRight className={cn(iconSize.base, 'ml-1')} />
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
