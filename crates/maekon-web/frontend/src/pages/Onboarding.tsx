/**
 * First-run onboarding, shown before the main shell. Five steps (#13734):
 * Intro → Permissions → Consent → AI → Ready.
 *
 * The AI step connects a provider with the same wizard and verification that
 * Settings uses, takes each AI consent on its own, and walks through the
 * restart a new provider needs. The current step survives that restart, so
 * setup resumes where it left off. Coaching stays opt-in in Settings (#5707)
 * and audio stays off by default, so neither needs a step of its own.
 */

import { useQueryClient } from '@tanstack/react-query'
import {
  Bell,
  Camera,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  CircleCheckBig,
  Footprints,
  History,
  Keyboard,
  Lightbulb,
  type LucideIcon,
  Mic,
  Monitor,
  Plug,
  Power,
  Rocket,
  RotateCcw,
  Search as SearchIcon,
  Shield,
  ShieldCheck,
  Sparkles,
} from 'lucide-react'
import { type ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  CONSENT_QUERY_KEY,
  type DesktopPermissionSnapshot,
  type DesktopPermissionState,
  fetchDesktopPermissionStatus,
  getConsent,
  requestDesktopNotificationPermission,
  requestDesktopScreenCapturePermission,
  setConsent,
} from '../api/client'
import type {
  AiCapabilityId,
  AiCapabilityReadiness,
  AiConsentField,
  AiReadinessReasonCode,
  ConsentPermissions,
  ConsentSnapshot,
  FeatureCapabilitySnapshot,
} from '../api/contracts'
import { SupportDetails } from '../components/SupportDetails'
import { Alert, Badge, Button, Checkbox, Spinner } from '../components/ui'
import { aiCapabilityReadiness, CHAT_CAPABILITY_IDS } from '../features/aiReadiness'
import { providerSurfaceById } from '../features/providerSurfaces'
import { invalidateAiReadinessSnapshotCache, useAiReadinessSnapshot } from '../hooks/useAiReadinessSnapshot'
import { type ErrorDisplay, useErrorDisplay } from '../i18n/errorDisplay'
import { colors, iconSize, motion, radius, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { IS_LINUX, IS_MAC, IS_TAURI, IS_WINDOWS } from '../utils/platform'
import ChatHttpVerification, { type VerificationOutcome } from './setting-tabs/ai-automation/ChatHttpVerification'
import ProviderWizard, { applyProviderWizardSelection, type ProviderDef } from './setting-tabs/ProviderWizard'
import { SettingsFormProvider, useSettingsFormContext } from './settings/SettingsFormContext'

interface OnboardingProps {
  onComplete: () => void
}

const TOTAL_STEPS = 5

/** Keeps the current step across the restart the AI step asks for (#13734). */
export const ONBOARDING_STEP_STORAGE_KEY = 'maekon.onboarding.step'

function readStoredStep(): number {
  try {
    const stored = Number(window.localStorage.getItem(ONBOARDING_STEP_STORAGE_KEY))
    return Number.isInteger(stored) && stored > 0 && stored < TOTAL_STEPS ? stored : 0
  } catch {
    return 0
  }
}

function storeStep(step: number) {
  try {
    if (step > 0) {
      window.localStorage.setItem(ONBOARDING_STEP_STORAGE_KEY, String(step))
    } else {
      window.localStorage.removeItem(ONBOARDING_STEP_STORAGE_KEY)
    }
  } catch {
    // Without storage, a restart starts setup from the first step again.
  }
}

// The first-run monitoring consent bundle. Superset of ConsentToggleSection's
// 6-field MONITORING_FIELDS by `ocr_processing` (#9631): without OCR consent,
// captured frames carry no text, so /search finds nothing and scene analysis
// is empty — the user reads that as "search is broken," not "a consent is
// off." OCR stays individually revocable here on the AI step and in Privacy.
// Every other high-sensitivity / additional opt-in field remains false.
const MONITORING_FIELDS = [
  'screen_capture',
  'ocr_processing',
  'window_title_collection',
  'app_usage_analytics',
  'process_monitoring',
  'input_activity',
  'telemetry',
] as const satisfies readonly (keyof ConsentPermissions)[]

const NO_PERMISSIONS: ConsentPermissions = {
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

/** Reads the consent record, or null when there is none to read (standalone preview). */
async function readConsent(): Promise<ConsentSnapshot | null> {
  try {
    const snapshot = await getConsent()
    return snapshot && typeof snapshot.status === 'string' ? snapshot : null
  } catch {
    return null
  }
}

/**
 * The opt-ins in force now. set_consent replaces the whole record, so every
 * write starts from these; an expired or unreadable record carries nothing over.
 */
function activePermissions(current: ConsentSnapshot | null): Partial<ConsentPermissions> {
  return current?.status === 'Valid' ? current.permissions : {}
}

function monitoringActive(snapshot: ConsentSnapshot | null): boolean {
  return snapshot?.status === 'Valid' && snapshot.permissions.screen_capture === true
}

/** The monitoring bundle on top of what is in force, so an AI consent granted first is kept. */
function buildMonitoringGrant(current: ConsentSnapshot | null): ConsentPermissions {
  const permissions: ConsentPermissions = { ...NO_PERMISSIONS, ...activePermissions(current) }
  for (const field of MONITORING_FIELDS) {
    permissions[field] = true
  }
  return permissions
}

async function grantMonitoring() {
  await setConsent(buildMonitoringGrant(await readConsent()))
  invalidateAiReadinessSnapshotCache()
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

/* ── Shared step layout ── */

interface StepNav {
  /** Absent on the first step. */
  onBack?: () => void
  onNext: () => void
  /** Ends setup at once. Only the first two steps offer it. */
  onSkipSetup: () => void
}

/** A scrolling body above a footer that stays in view, so no action is clipped at 1280×800. */
function StepFrame({ nav, actions, children }: { nav: StepNav; actions: ReactNode; children: ReactNode }) {
  const { t } = useTranslation()
  return (
    <>
      <div className="min-h-0 flex-1 overflow-y-auto px-8 pb-6">{children}</div>
      <div className="flex items-center justify-between gap-3 border-muted border-t px-8 py-4">
        <div>
          {nav.onBack && (
            <Button variant="ghost" size="md" onClick={nav.onBack} data-testid="onboarding-back">
              <ChevronLeft className={cn(iconSize.base, 'mr-1')} />
              {t('onboarding.back')}
            </Button>
          )}
        </div>
        <div className="flex flex-wrap items-center justify-end gap-3">{actions}</div>
      </div>
    </>
  )
}

function StepHeader({ icon: Icon, title, description }: { icon: LucideIcon; title: string; description?: string }) {
  return (
    <div className="mb-6 flex flex-col items-center text-center">
      <div className={cn('mb-4 flex items-center justify-center rounded-full bg-brand-signal/15 p-3', motion.opacity)}>
        <Icon className={cn(iconSize.hero, 'text-brand-text')} aria-hidden="true" />
      </div>
      <h2 className={cn(typography.h1, colors.text.primary, 'mb-2')}>{title}</h2>
      {description && <p className={cn(typography.body, colors.text.secondary, 'max-w-md')}>{description}</p>}
    </div>
  )
}

function NextButton({ onClick, disabled = false }: { onClick: () => void; disabled?: boolean }) {
  const { t } = useTranslation()
  return (
    <Button variant="primary" size="md" disabled={disabled} onClick={onClick} data-testid="onboarding-next">
      {t('onboarding.next')}
      <ChevronRight className={cn(iconSize.base, 'ml-1')} />
    </Button>
  )
}

function SkipSetupButton({ onClick }: { onClick: () => void }) {
  const { t } = useTranslation()
  return (
    <Button variant="ghost" size="md" data-testid="onboarding-skip" onClick={onClick}>
      {t('onboarding.skip')}
    </Button>
  )
}

/* ── Step 1: what Maekon does ── */

const INTRO_POINTS = [
  { key: 'capture', icon: History },
  { key: 'resume', icon: Footprints },
  { key: 'search', icon: SearchIcon },
] as const

function StepIntro({ nav }: { nav: StepNav }) {
  const { t } = useTranslation()
  return (
    <StepFrame
      nav={nav}
      actions={
        <>
          <SkipSetupButton onClick={nav.onSkipSetup} />
          <NextButton onClick={nav.onNext} />
        </>
      }
    >
      <StepHeader icon={Rocket} title={t('onboarding.step1Title')} description={t('onboarding.step1Desc')} />
      <ul className="mx-auto flex w-full max-w-md flex-col gap-3">
        {INTRO_POINTS.map(({ key, icon: Icon }) => (
          <li key={key} className="flex items-start gap-3 rounded-lg bg-surface-muted px-4 py-3 text-left">
            <div className="flex items-center justify-center rounded-md bg-brand-signal/15 p-2">
              <Icon className={cn(iconSize.base, 'text-brand-text')} aria-hidden="true" />
            </div>
            <div className="min-w-0">
              <p className={cn(typography.label, colors.text.primary)}>{t(`onboarding.intro.${key}Title`)}</p>
              <p className={cn(typography.small, colors.text.secondary, 'mt-1')}>{t(`onboarding.intro.${key}Desc`)}</p>
            </div>
          </li>
        ))}
      </ul>
      <p className={cn(typography.small, colors.text.tertiary, 'mx-auto mt-4 max-w-md text-center')}>
        {t('onboarding.intro.privacyNote')}
      </p>
    </StepFrame>
  )
}

/* ── Step 2: OS permissions ── */

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

function StepPermissions({
  nav,
  canAdvance,
  onReadyChange,
}: {
  nav: StepNav
  canAdvance: boolean
  onReadyChange: (ready: boolean) => void
}) {
  const { t } = useTranslation()
  const describeError = useErrorDisplay()
  const [permissionStatus, setPermissionStatus] = useState<DesktopPermissionSnapshot | null>(null)
  const [loading, setLoading] = useState(IS_TAURI)
  const [error, setError] = useState<ErrorDisplay | null>(null)
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
      setError(describeError(nextError, t('onboarding.step2PermissionCheckFailed')))
    } finally {
      setLoading(false)
    }
  }, [describeError, onReadyChange, t])

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
        setError(describeError(nextError, t('onboarding.step2SystemSettingsOpenFailed')))
      } finally {
        setActionLoading(null)
      }
    },
    [describeError, t],
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
      setError(describeError(nextError, t('onboarding.step2SystemSettingsOpenFailed')))
    } finally {
      setActionLoading(null)
    }
  }, [describeError, t])

  const handleRequestNotificationPermission = useCallback(async () => {
    setActionLoading('notifications')
    setError(null)
    try {
      const snapshot = await requestDesktopNotificationPermission()
      setPermissionStatus(snapshot)
    } catch (nextError) {
      setError(describeError(nextError, t('onboarding.step2NotificationRequestFailed')))
    } finally {
      setActionLoading(null)
    }
  }, [describeError, t])

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
    <StepFrame
      nav={nav}
      actions={
        <>
          <SkipSetupButton onClick={nav.onSkipSetup} />
          <NextButton onClick={nav.onNext} disabled={!canAdvance} />
        </>
      }
    >
      <StepHeader icon={Shield} title={t('onboarding.step2Title')} description={description} />

      <div className="flex flex-col items-center text-center">
        <div className="mb-4 flex w-full justify-end">
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
          <div className="mb-4 w-full">
            <Alert variant="error" title={t('settings.permissionCheckFailedTitle')}>
              <p>{error.text}</p>
              <SupportDetails detail={error.detail} />
            </Alert>
          </div>
        )}

        <div className="flex w-full flex-col gap-3">
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
          <div className="mt-4 w-full">
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
          <div className="mt-4 w-full">
            <Alert variant="info" title={t('onboarding.step2ScreenCaptureRestartTitle')}>
              <p>{t('onboarding.step2ScreenCaptureRestartDesc')}</p>
            </Alert>
          </div>
        )}

        {!IS_MAC && permissionRows.length > 0 && (
          <div className="mt-4 w-full">
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
    </StepFrame>
  )
}

/* ── Step 3: monitoring consent (#4629 A.2 task 4) ── */

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

function StepConsent({ nav }: { nav: StepNav }) {
  const { t } = useTranslation()
  const describeError = useErrorDisplay()
  const queryClient = useQueryClient()
  const [granted, setGranted] = useState(false)
  const [granting, setGranting] = useState(false)
  const [error, setError] = useState<ErrorDisplay | null>(null)

  // Someone who comes back here, or resumes after a restart, sees the grant already in force.
  useEffect(() => {
    let cancelled = false
    void readConsent().then((snapshot) => {
      if (!cancelled && monitoringActive(snapshot)) setGranted(true)
    })
    return () => {
      cancelled = true
    }
  }, [])

  // #4629: set_consent runs only on this explicit click, after the disclosure above it.
  // The one primary action grants and moves on; continuing without it is a
  // separate, plainly labelled choice (#13734).
  const grantAndContinue = async () => {
    setGranting(true)
    setError(null)
    try {
      await grantMonitoring()
      void queryClient.invalidateQueries({ queryKey: CONSENT_QUERY_KEY })
      setGranted(true)
      setGranting(false)
      nav.onNext()
    } catch (nextError) {
      setError(describeError(nextError))
      setGranting(false)
    }
  }

  return (
    <StepFrame
      nav={nav}
      actions={
        granted ? (
          <NextButton onClick={nav.onNext} />
        ) : (
          <>
            <Button
              variant="ghost"
              size="md"
              disabled={granting}
              onClick={nav.onNext}
              data-testid="onboarding-consent-decline"
            >
              {t('onboarding.consent.continueWithout')}
            </Button>
            <Button
              variant="primary"
              size="md"
              isLoading={granting}
              onClick={() => void grantAndContinue()}
              data-testid="onboarding-consent-grant"
            >
              <ShieldCheck className={cn(iconSize.base, 'mr-2')} />
              {granting ? t('onboarding.consent.granting') : t('onboarding.consent.grantAndContinue')}
            </Button>
          </>
        )
      }
    >
      <StepHeader
        icon={ShieldCheck}
        title={t('onboarding.consent.title')}
        description={t('onboarding.consent.description')}
      />

      {granted ? (
        <div className="mx-auto w-full max-w-md" data-testid="onboarding-consent-granted">
          <Alert variant="success" title={t('onboarding.consent.granted')}>
            <p>{t('onboarding.consent.grantedNote')}</p>
          </Alert>
        </div>
      ) : (
        <div className="mx-auto w-full max-w-md text-left">
          <p className={cn(typography.small, colors.text.secondary, 'mb-2')}>
            {t('onboarding.consent.collectedTitle')}
          </p>
          <ul className="mb-4 grid list-inside list-disc grid-cols-1 gap-x-4 text-content-tertiary text-xs sm:grid-cols-2">
            {CONSENT_COLLECTED_KEYS.map((key) => (
              <li key={key}>{t(`privacy.consent.monitoring.collected.${key}`)}</li>
            ))}
          </ul>

          {error && (
            <Alert variant="error" className="mb-4" title={t('onboarding.consent.error')}>
              <p>{error.text}</p>
              <SupportDetails detail={error.detail} />
            </Alert>
          )}

          <p className={cn(typography.small, colors.text.secondary)} data-testid="onboarding-consent-decline-note">
            {t('onboarding.consent.declineNote', { label: t('privacy.consent.monitoring.label') })}
          </p>
          <p className="mt-3 text-content-tertiary text-xs">{t('onboarding.consent.osNote')}</p>
        </div>
      )}
    </StepFrame>
  )
}

/* ── Step 4: AI connection (#13734) ── */

// One row per consent, never bundled: each write changes only its own field,
// so an item left unchecked is never saved as granted.
const AI_CONSENTS = [
  {
    field: 'full_text_extraction',
    labelKey: 'privacy.consent.fullText.label',
    descriptionKey: 'onboarding.ai.consent.fullTextDesc',
    testId: 'onboarding-ai-consent-full-text',
  },
  {
    field: 'ocr_processing',
    labelKey: 'privacy.consent.ocrProcessing.label',
    descriptionKey: 'onboarding.ai.consent.ocrDesc',
    testId: 'onboarding-ai-consent-ocr',
  },
  {
    // Coaching alerts (`coaching.enabled`) stay off; Settings → Coaching turns them on (#5707).
    field: 'activity_pattern_learning',
    labelKey: 'privacy.consent.patternLearning.label',
    descriptionKey: 'onboarding.ai.consent.patternLearningDesc',
    testId: 'onboarding-ai-consent-pattern-learning',
  },
] as const satisfies readonly { field: AiConsentField; labelKey: string; descriptionKey: string; testId: string }[]

/** `chat.http_api` blockers one verification call settles; the same set ChatHttpVerification offers (#12530). */
const VERIFIABLE_REASONS: ReadonlySet<AiReadinessReasonCode> = new Set([
  'provider_invocation_unverified',
  'model_availability_unverified',
])

/** Blockers the wizard fixes by saving a provider and its key. */
const PROVIDER_NEEDED_REASONS: ReadonlySet<AiReadinessReasonCode> = new Set([
  'access_mode_mismatch',
  'endpoint_or_profile_required',
  'http_chat_settings_resave_required',
  'provider_auth_required',
])

/** Blockers that no provider choice fixes in this build. */
const UNAVAILABLE_REASONS: ReadonlySet<AiReadinessReasonCode> = new Set([
  'compiled_capability_missing',
  'runtime_flag_disabled',
  'privacy_gate_unavailable',
  'egress_gate_unavailable',
  'budget_gate_unavailable',
  'audit_gate_unavailable',
])

const AI_FEATURE_ROWS = [
  { capabilityId: 'ocr.suggestion_analysis', copyKey: 'suggestions' },
  { capabilityId: 'segment_summary', copyKey: 'segmentSummary' },
  { capabilityId: 'daily_narrative', copyKey: 'dailySummary' },
] as const satisfies readonly { capabilityId: AiCapabilityId; copyKey: string }[]

function chatConnected(snapshot: FeatureCapabilitySnapshot | null | undefined): boolean {
  return CHAT_CAPABILITY_IDS.some((capabilityId) => aiCapabilityReadiness(snapshot, capabilityId).status === 'ready')
}

/** A saved provider that only a restart can apply; checking it before then returns `restart_required`. */
function awaitsRestart(readiness: AiCapabilityReadiness): boolean {
  if (readiness.reason_code === 'restart_required') return true
  const { apply_pending, apply_requirement } = readiness.dimensions
  return apply_pending && apply_requirement === 'restart' && VERIFIABLE_REASONS.has(readiness.reason_code)
}

function canReadReadiness(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

type AiPhase =
  | 'loading'
  | 'consent'
  | 'unavailable'
  | 'saving'
  | 'provider'
  | 'restart'
  | 'verify'
  | 'blocked'
  | 'connected'

function StepAi({ nav }: { nav: StepNav }) {
  // Onboarding renders outside the shell's provider, so the step brings its own
  // and saves through the same form Settings uses.
  return (
    <SettingsFormProvider>
      <StepAiBody nav={nav} />
    </SettingsFormProvider>
  )
}

function StatusLine({ children, testId }: { children: ReactNode; testId?: string }) {
  return (
    <div className={cn('flex items-center gap-2', typography.body, colors.text.secondary)} data-testid={testId}>
      <Spinner size="sm" />
      <span>{children}</span>
    </div>
  )
}

function StepAiBody({ nav }: { nav: StepNav }) {
  const { t } = useTranslation()
  const describeError = useErrorDisplay()
  const queryClient = useQueryClient()
  const { form, data } = useSettingsFormContext()
  const { formData, saveSettings } = form
  const snapshot = useAiReadinessSnapshot()
  const consentHeadingId = useId()
  const providerHeadingId = useId()

  const [consent, setConsentRecord] = useState<ConsentSnapshot | null>(null)
  const [consentLoaded, setConsentLoaded] = useState(false)
  const [writingField, setWritingField] = useState<AiConsentField | null>(null)
  const [consentError, setConsentError] = useState<{ field: AiConsentField; display: ErrorDisplay } | null>(null)
  const [outcome, setOutcome] = useState<VerificationOutcome | null>(null)
  const [choosingProvider, setChoosingProvider] = useState(false)
  const [pendingSave, setPendingSave] = useState<{ surfaceId: string } | null>(null)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<ErrorDisplay | null>(null)
  const [quitting, setQuitting] = useState(false)
  const [quitFailed, setQuitFailed] = useState(false)

  const refreshConsent = useCallback(async () => {
    setConsentRecord(await readConsent())
    setConsentLoaded(true)
  }, [])

  useEffect(() => {
    void refreshConsent()
  }, [refreshConsent])

  const isGranted = (field: AiConsentField) => consent?.status === 'Valid' && consent.permissions[field] === true

  const toggleConsent = async (field: AiConsentField, next: boolean) => {
    setWritingField(field)
    setConsentError(null)
    try {
      // Re-read first: another surface may have changed the record since this step loaded.
      const current = await readConsent()
      await setConsent({ ...NO_PERMISSIONS, ...activePermissions(current), [field]: next })
      invalidateAiReadinessSnapshotCache()
      void queryClient.invalidateQueries({ queryKey: CONSENT_QUERY_KEY })
      void queryClient.invalidateQueries({ queryKey: ['feature-capabilities'] })
      setOutcome(null)
    } catch (error) {
      setConsentError({ field, display: describeError(error) })
    } finally {
      // The re-read decides what the box shows, not the click.
      await refreshConsent()
      setWritingField(null)
    }
  }

  const handleProviderSelect = (provider: ProviderDef, apiKey: string) => {
    setSaveError(null)
    // The form ignores a surface the catalog lacks; say so instead of saving nothing.
    if (!providerSurfaceById(data.providerCatalog, provider.surfaceId)) {
      setSaveError({ text: t('onboarding.ai.providerUnsupported'), detail: null })
      return
    }
    applyProviderWizardSelection(form, provider, apiKey)
    // #8059 G2b keeps summaries and suggestions an explicit opt-in. This save is
    // that opt-in: the hint above the wizard says so before the click, and
    // continuing without AI leaves all four flags at their default (off).
    form.setFormData((current) =>
      current
        ? {
            ...current,
            analysis: {
              ...current.analysis,
              enabled: true,
              tiered_memory_enabled: true,
              embedding_enabled: true,
              llm_summary_enabled: true,
            },
          }
        : current,
    )
    setOutcome(null)
    setChoosingProvider(false)
    setPendingSave({ surfaceId: provider.surfaceId })
  }

  // The selection lands in the form on the next render; save once it has.
  useEffect(() => {
    if (!pendingSave || !formData || formData.ai_provider.llm_api?.surface_id !== pendingSave.surfaceId) return
    setPendingSave(null)
    setSaving(true)
    saveSettings(formData)
      .catch((error: unknown) => setSaveError(describeError(error)))
      .finally(() => setSaving(false))
  }, [pendingSave, formData, saveSettings, describeError])

  const quitMaekon = async () => {
    setQuitting(true)
    setQuitFailed(false)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('request_app_quit')
    } catch {
      setQuitFailed(true)
      setQuitting(false)
    }
  }

  const fullTextGranted = isGranted('full_text_extraction')
  const chat = snapshot ? aiCapabilityReadiness(snapshot, 'chat.http_api') : null
  const connected = chatConnected(snapshot) || outcome === 'verified'

  let phase: AiPhase
  if (!consentLoaded) phase = 'loading'
  else if (!fullTextGranted) phase = 'consent'
  else if (pendingSave || saving) phase = 'saving'
  else if (!formData) phase = data.settingsLoading ? 'loading' : 'unavailable'
  else if (choosingProvider) phase = 'provider'
  else if (connected) phase = 'connected'
  else if (outcome === 'restart_required' || (chat && awaitsRestart(chat))) phase = 'restart'
  else if (!chat) phase = canReadReadiness() ? 'loading' : 'provider'
  else if (VERIFIABLE_REASONS.has(chat.reason_code)) phase = 'verify'
  else if (PROVIDER_NEEDED_REASONS.has(chat.reason_code)) phase = 'provider'
  else phase = 'blocked'

  // At 1280×800 the consent rows fill the first screen, so a new provider state
  // or check result would land below the fold. Bring the end of the provider
  // part into view when it changes, but not when a consent toggle only reloads
  // the state already shown.
  const providerEndRef = useRef<HTMLDivElement>(null)
  const lastSettledView = useRef<string | null>(null)
  // The feature list arrives with the refreshed readiness, after the connection itself.
  const settledView = phase === 'connected' && snapshot ? 'connected-features' : phase
  useEffect(() => {
    if (settledView === 'loading') return
    const previous = lastSettledView.current
    lastSettledView.current = settledView
    if (settledView === 'consent' || settledView === previous) return
    providerEndRef.current?.scrollIntoView?.({ block: 'nearest' })
  }, [settledView])
  useEffect(() => {
    if (outcome) providerEndRef.current?.scrollIntoView?.({ block: 'nearest' })
  }, [outcome])

  // Without a saved endpoint the form resolves a default surface, which is not a saved provider.
  const savedProviderName = formData?.ai_provider.llm_api?.surface_id
    ? form.resolveEndpointSurface('llm_api')?.display_name
    : undefined
  const changeProviderButton = (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      onClick={() => setChoosingProvider(true)}
      data-testid="onboarding-ai-change-provider"
    >
      {t('onboarding.ai.changeProvider')}
    </Button>
  )
  const wizard = (
    <>
      {saveError && (
        <Alert variant="error" title={t('onboarding.ai.saveFailed')} data-testid="onboarding-ai-save-error">
          <p>{saveError.text}</p>
          <SupportDetails detail={saveError.detail} />
        </Alert>
      )}
      <p className={cn(typography.small, colors.text.secondary)}>{t('onboarding.ai.providerHint')}</p>
      <ProviderWizard onSelect={handleProviderSelect} />
      {choosingProvider && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => setChoosingProvider(false)}
          data-testid="onboarding-ai-change-cancel"
        >
          {t('common.cancel')}
        </Button>
      )}
    </>
  )

  let providerBody: ReactNode
  switch (phase) {
    case 'loading':
      providerBody = <StatusLine testId="onboarding-ai-checking">{t('onboarding.ai.checking')}</StatusLine>
      break
    case 'consent':
      providerBody = (
        <div
          className="flex items-start gap-3 rounded-lg border border-muted bg-surface-muted/80 p-4"
          data-testid="onboarding-ai-needs-consent"
        >
          <Plug className={cn(iconSize.base, 'mt-0.5 shrink-0 text-content-secondary')} aria-hidden="true" />
          <p className={cn(typography.body, colors.text.secondary)}>
            {t('onboarding.ai.providerNeedsConsent', { label: t('privacy.consent.fullText.label') })}
          </p>
        </div>
      )
      break
    case 'unavailable':
      providerBody = (
        <Alert variant="warning" title={t('onboarding.ai.loadFailed')} data-testid="onboarding-ai-load-failed">
          <p>{t('onboarding.ai.loadFailedDesc')}</p>
        </Alert>
      )
      break
    case 'saving':
      providerBody = <StatusLine testId="onboarding-ai-saving">{t('onboarding.ai.saving')}</StatusLine>
      break
    case 'provider':
      providerBody = wizard
      break
    case 'restart':
      providerBody = (
        <div
          className="space-y-3 rounded-lg border border-muted bg-surface-muted/80 p-4"
          data-testid="onboarding-ai-restart"
        >
          <div className="flex items-start gap-3">
            <Power className={cn(iconSize.md, 'mt-0.5 shrink-0 text-brand-text')} aria-hidden="true" />
            <div className="min-w-0 space-y-1">
              <p className={cn(typography.label, colors.text.primary)}>{t('onboarding.ai.restartTitle')}</p>
              <p className={cn(typography.body, colors.text.secondary)}>{t('onboarding.ai.restartDesc')}</p>
              {savedProviderName && (
                <p className={cn(typography.small, colors.text.tertiary)}>
                  {t('onboarding.ai.savedProvider', { name: savedProviderName })}
                </p>
              )}
            </div>
          </div>
          {quitFailed && (
            <Alert variant="error" data-testid="onboarding-ai-quit-failed">
              <p>{t('onboarding.ai.quitFailed')}</p>
            </Alert>
          )}
          {changeProviderButton}
        </div>
      )
      break
    case 'verify':
      providerBody = (
        <>
          <ChatHttpVerification snapshot={snapshot} hasUnsavedChanges={false} onOutcome={setOutcome} />
          {changeProviderButton}
        </>
      )
      break
    case 'blocked': {
      // No provider choice fixes a build or gate that lacks AI, so only the other blockers offer the wizard.
      const unavailable = chat ? UNAVAILABLE_REASONS.has(chat.reason_code) : false
      providerBody = (
        <>
          {chat && (
            <Alert variant="warning" title={t('onboarding.ai.blockedTitle')} data-testid="onboarding-ai-blocked">
              <p>{t(unavailable ? 'onboarding.ai.unavailable' : 'onboarding.ai.unreachable')}</p>
              <code className={cn(typography.nano, colors.text.tertiary)}>{chat.reason_code}</code>
            </Alert>
          )}
          {!unavailable && wizard}
        </>
      )
      break
    }
    case 'connected':
      providerBody = (
        <>
          <Alert variant="success" title={t('onboarding.ai.connectedTitle')} data-testid="onboarding-ai-connected">
            <p>{t('onboarding.ai.connectedDesc')}</p>
          </Alert>
          {snapshot && (
            <ul className="space-y-2" data-testid="onboarding-ai-features">
              {AI_FEATURE_ROWS.map(({ capabilityId, copyKey }) => {
                const readiness = aiCapabilityReadiness(snapshot, capabilityId)
                const missing = readiness.dimensions.consent
                  .filter((entry) => !entry.granted)
                  .map((entry) => t(AI_CONSENTS.find((row) => row.field === entry.field)?.labelKey ?? entry.field))
                const status =
                  readiness.status === 'ready'
                    ? 'on'
                    : readiness.reason_code === 'consent_required' && missing.length > 0
                      ? 'needsConsent'
                      : readiness.reason_code === 'restart_required'
                        ? 'afterRestart'
                        : 'inSettings'
                return (
                  <li
                    key={capabilityId}
                    data-capability-id={capabilityId}
                    data-status={status}
                    className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 rounded-lg bg-surface-muted px-4 py-2"
                  >
                    <span className={cn(typography.body, colors.text.primary)}>
                      {t(`onboarding.ai.features.${copyKey}`)}
                    </span>
                    <span
                      className={cn(
                        typography.small,
                        status === 'on' ? 'text-semantic-success' : colors.text.secondary,
                      )}
                    >
                      {t(`onboarding.ai.featureStatus.${status}`, { items: missing.join(', ') })}
                    </span>
                  </li>
                )
              })}
            </ul>
          )}
        </>
      )
      break
  }

  const actions =
    phase === 'connected' ? (
      <NextButton onClick={nav.onNext} />
    ) : phase === 'restart' ? (
      <>
        <Button variant="ghost" size="md" onClick={nav.onNext} data-testid="onboarding-ai-restart-later">
          {t('onboarding.ai.restartLater')}
        </Button>
        <Button
          variant="primary"
          size="md"
          isLoading={quitting}
          onClick={() => void quitMaekon()}
          data-testid="onboarding-ai-quit"
        >
          <Power className={cn(iconSize.base, 'mr-2')} />
          {t('onboarding.ai.quitButton')}
        </Button>
      </>
    ) : (
      <>
        <Button variant="ghost" size="md" onClick={nav.onNext} data-testid="onboarding-ai-skip">
          {t('onboarding.ai.withoutButton')}
        </Button>
        {/* A failed or missing check keeps Next off; only a real connection opens it. */}
        <NextButton onClick={nav.onNext} disabled />
      </>
    )

  return (
    <StepFrame nav={nav} actions={actions}>
      <StepHeader icon={Sparkles} title={t('onboarding.ai.title')} description={t('onboarding.ai.description')} />
      <div className="mx-auto w-full max-w-xl space-y-6 text-left">
        <section aria-labelledby={consentHeadingId} className="space-y-3">
          <div>
            <h3 id={consentHeadingId} className={cn(typography.label, colors.text.primary)}>
              {t('onboarding.ai.consentTitle')}
            </h3>
            <p className={cn(typography.small, colors.text.secondary, 'mt-1')}>{t('onboarding.ai.consentHint')}</p>
          </div>
          {AI_CONSENTS.map(({ field, labelKey, descriptionKey, testId }) => (
            <div key={field} className="rounded-lg border border-muted bg-surface-inset p-3">
              <Checkbox
                data-testid={testId}
                label={t(labelKey)}
                description={t(descriptionKey)}
                checked={isGranted(field)}
                disabled={!consentLoaded || writingField !== null}
                onChange={(event) => void toggleConsent(field, event.target.checked)}
              />
              {field === 'ocr_processing' && IS_LINUX && (
                <p className="mt-2 text-content-tertiary text-xs">
                  {t('privacy.consent.ocrProcessing.linuxUnsupported')}
                </p>
              )}
              {consentError?.field === field && (
                <Alert variant="error" className="mt-2" title={t('onboarding.ai.consentError')}>
                  <p>{consentError.display.text}</p>
                  <SupportDetails detail={consentError.display.detail} />
                </Alert>
              )}
            </div>
          ))}
        </section>

        <section
          aria-labelledby={providerHeadingId}
          className="space-y-3"
          data-testid="onboarding-ai-provider"
          data-phase={phase}
        >
          <h3 id={providerHeadingId} className={cn(typography.label, colors.text.primary)}>
            {t('onboarding.ai.providerTitle')}
          </h3>
          {providerBody}
          <div ref={providerEndRef} />
        </section>

        {phase !== 'connected' && (
          <p className={cn(typography.small, colors.text.tertiary)} data-testid="onboarding-ai-without-note">
            {t('onboarding.ai.withoutNote')}
          </p>
        )}
      </div>
    </StepFrame>
  )
}

/* ── Step 5: ready ── */

type ReadyAiStatus = 'connected' | 'restartPending' | 'off'

function readyAiStatus(snapshot: FeatureCapabilitySnapshot | null | undefined): ReadyAiStatus | null {
  if (!snapshot) return null
  if (chatConnected(snapshot)) return 'connected'
  return awaitsRestart(aiCapabilityReadiness(snapshot, 'chat.http_api')) ? 'restartPending' : 'off'
}

function ReadyNote({
  icon: Icon,
  children,
  testId,
  status,
}: {
  icon: LucideIcon
  children: ReactNode
  testId?: string
  status?: string
}) {
  return (
    <div
      role="note"
      data-testid={testId}
      data-status={status}
      className={cn('flex items-start gap-2 rounded-lg bg-surface-muted px-4 py-3 text-left', motion.colors)}
    >
      <Icon className={cn(iconSize.sm, 'mt-0.5 flex-shrink-0 text-brand-text')} aria-hidden="true" />
      <p className={cn(typography.small, colors.text.secondary)}>{children}</p>
    </div>
  )
}

function StepReady({ nav, onFinish }: { nav: StepNav; onFinish: () => void }) {
  const { t } = useTranslation()
  const describeError = useErrorDisplay()
  const shortcut = IS_MAC ? '⌘K' : 'Ctrl+K'
  const snapshot = useAiReadinessSnapshot()
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
  // #9811: the consent step does not gate Next — consent must stay freely
  // refusable — so a user can reach this screen without granting. When that
  // happens the app collects nothing (`get_capture_status` reports
  // `consent_granted:false, permitted:false`) and the timeline is empty
  // forever. Declaring "everything is set up" there is simply false, and it is
  // the reason a new install looks broken rather than switched off.
  //
  // Consent is NOT forced here. The claim is corrected and the grant is offered
  // in place, so refusing stays a real option that is honestly described.
  const [consentGranted, setConsentGranted] = useState<boolean | null>(null)
  const [granting, setGranting] = useState(false)
  const [grantError, setGrantError] = useState<ErrorDisplay | null>(null)
  const refreshConsent = useCallback(async () => {
    const record = await readConsent()
    // Standalone / non-Tauri preview has no consent backend to report on.
    // Say nothing rather than accuse the user.
    setConsentGranted(record ? monitoringActive(record) : null)
  }, [])
  useEffect(() => {
    void refreshConsent()
  }, [refreshConsent])

  const grantHere = async () => {
    setGranting(true)
    setGrantError(null)
    try {
      await grantMonitoring()
      await refreshConsent()
    } catch (error) {
      setGrantError(describeError(error))
    } finally {
      setGranting(false)
    }
  }

  const collectionOff = consentGranted === false
  const aiStatus = readyAiStatus(snapshot)

  return (
    <StepFrame
      nav={nav}
      actions={
        <Button variant="primary" size="md" onClick={onFinish} data-testid="onboarding-complete">
          {t('onboarding.complete')}
        </Button>
      }
    >
      <StepHeader
        icon={Rocket}
        title={collectionOff ? t('onboarding.readyCollectionOffTitle') : t('onboarding.step4Title')}
        description={collectionOff ? t('onboarding.readyCollectionOffDesc') : t('onboarding.ready.desc')}
      />
      <div className="mx-auto flex w-full max-w-md flex-col gap-3">
        {collectionOff && (
          <div className="space-y-2" data-testid="onboarding-ready-collection-off">
            <Button
              type="button"
              variant="secondary"
              size="md"
              className="w-full"
              isLoading={granting}
              onClick={() => void grantHere()}
            >
              {t('onboarding.readyGrantNow')}
            </Button>
            {grantError && (
              <Alert variant="error" title={t('onboarding.consent.error')}>
                <p>{grantError.text}</p>
                <SupportDetails detail={grantError.detail} />
              </Alert>
            )}
          </div>
        )}
        {aiStatus && (
          <ReadyNote
            icon={aiStatus === 'connected' ? CircleCheckBig : Sparkles}
            testId="onboarding-ready-ai"
            status={aiStatus}
          >
            {t(`onboarding.ready.ai.${aiStatus}`)}
          </ReadyNote>
        )}
        {/* #8686 AC5: set the first-run expectation — where the first
            source-backed recovery card comes from. #12521: it appears when the
            user presses "Find my next step" in the tracking panel, not on a
            timer, so the copy names that button instead of promising ~15 min. */}
        <ReadyNote icon={Footprints}>
          {t('onboarding.step4RecoveryCardHint')}
          {aiStatus !== 'connected' && ` ${t('onboarding.ready.recoveryNeedsAi')}`}
        </ReadyNote>
        <ReadyNote icon={Lightbulb}>
          {t('onboarding.step4SuggestionsShortcut', { shortcut: suggestionsShortcut })}
        </ReadyNote>
        <ReadyNote icon={Lightbulb}>{t('onboarding.step4Tip', { shortcut })}</ReadyNote>
      </div>
    </StepFrame>
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
  const [step, setStep] = useState(readStoredStep)
  const [permissionStepReady, setPermissionStepReady] = useState(!IS_TAURI || !IS_MAC)

  useEffect(() => {
    storeStep(step)
  }, [step])

  const finish = useCallback(async () => {
    storeStep(0)
    await invokeCommand('complete_onboarding')
    onComplete()
  }, [onComplete])

  const nav: StepNav = {
    onBack: step > 0 ? () => setStep((current) => Math.max(0, current - 1)) : undefined,
    onNext: () => setStep((current) => Math.min(TOTAL_STEPS - 1, current + 1)),
    onSkipSetup: () => void finish(),
  }

  return (
    <div className="flex h-screen items-center justify-center overflow-hidden bg-surface-sunken p-4">
      <div
        className={cn('flex max-h-full w-full max-w-2xl flex-col', radius.lg, 'bg-surface-elevated shadow-xl')}
        data-testid="onboarding-card"
        data-step={step}
      >
        <div className="flex justify-center px-8 pt-6 pb-4">
          <StepDots current={step} total={TOTAL_STEPS} />
        </div>
        {step === 0 && <StepIntro nav={nav} />}
        {step === 1 && (
          <StepPermissions nav={nav} canAdvance={permissionStepReady} onReadyChange={setPermissionStepReady} />
        )}
        {step === 2 && <StepConsent nav={nav} />}
        {step === 3 && <StepAi nav={nav} />}
        {step === 4 && <StepReady nav={nav} onFinish={() => void finish()} />}
      </div>
    </div>
  )
}
