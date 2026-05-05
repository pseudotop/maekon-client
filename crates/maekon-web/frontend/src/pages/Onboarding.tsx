/**
 * First-run onboarding page — 4-step guide shown before the main shell.
 */

import {
  Bell,
  Camera,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  CircleCheckBig,
  Cpu,
  Lightbulb,
  Monitor,
  Rocket,
  RotateCcw,
  Shield,
} from 'lucide-react'
import { type ReactNode, useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  type DesktopPermissionSnapshot,
  type DesktopPermissionState,
  fetchDesktopPermissionStatus,
  requestDesktopNotificationPermission,
} from '../api/client'
import { Alert, Badge, Button } from '../components/ui'
import { colors, iconSize, motion, radius, typography } from '../styles/tokens'
import { cn } from '../utils/cn'
import { IS_LINUX, IS_MAC, IS_TAURI, IS_WINDOWS } from '../utils/platform'

interface OnboardingProps {
  onComplete: () => void
}

const TOTAL_STEPS = 4

async function invokeCommand(cmd: string) {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke(cmd)
  } catch {
    // Standalone / dev mode — no Tauri runtime
  }
}

async function openPermissionSettings(permissionKind: 'accessibility' | 'screen_capture') {
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
  id: 'accessibility' | 'screen_capture' | 'notifications'
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
  const [actionLoading, setActionLoading] = useState<
    'accessibility' | 'screen_capture' | 'notifications' | 'refresh' | null
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
    async (kind: 'accessibility' | 'screen_capture') => {
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
            onClick: () => void handleOpenPermissionSettings('screen_capture'),
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

function FeatureItem({ icon, label }: { icon: ReactNode; label: string }) {
  return (
    <div className={cn('flex items-center gap-3 rounded-lg bg-surface-muted px-4 py-3', motion.colors)}>
      <div className="flex items-center justify-center rounded-md bg-brand-signal/15 p-2">{icon}</div>
      <span className={cn(typography.body, typography.weight.medium, colors.text.primary)}>{label}</span>
    </div>
  )
}

function StepFeatures() {
  const { t } = useTranslation()
  const iconCls = cn(iconSize.base, 'text-brand-text')
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
      </div>
    </div>
  )
}

function StepReady() {
  const { t } = useTranslation()
  const shortcut = IS_MAC ? '⌘K' : 'Ctrl+K'
  return (
    <div className="flex flex-col items-center text-center">
      <div className={cn('mb-6 flex items-center justify-center rounded-full bg-brand-signal/15 p-4', motion.opacity)}>
        <Rocket className={cn(iconSize.hero, 'text-brand-text')} />
      </div>
      <h2 className={cn(typography.h1, colors.text.primary, 'mb-3')}>{t('onboarding.step4Title')}</h2>
      <p className={cn(typography.body, colors.text.secondary, 'mb-4 max-w-sm')}>{t('onboarding.step4Desc')}</p>
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
  const steps = Array.from({ length: total }, (_, i) => ({ id: `step-${i + 1}`, index: i }))
  return (
    <fieldset className="m-0 flex items-center gap-2 border-none p-0" aria-label="Step indicator">
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

  const handleSkip = useCallback(() => {
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
          {step === 2 && <StepFeatures />}
          {step === 3 && <StepReady />}
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
            <Button variant="ghost" size="md" onClick={handleSkip}>
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
