import {
  Activity,
  Archive,
  BellDot,
  Camera,
  CheckCircle2,
  Clock3,
  ExternalLink,
  Eye,
  FileClock,
  Lock,
  Settings,
  ShieldCheck,
  Sparkles,
} from 'lucide-react'
import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { cn } from '../utils/cn'

export type TrayDashboardCaptureState = 'active' | 'paused' | 'local'
export type TrayDashboardProviderStatus = 'connected' | 'unavailable' | 'blocked'
export type TrayDashboardRedactionState = 'clean' | 'redacted' | 'blocked'
export type TrayDashboardExternalEgressState = 'allowed' | 'blocked'
export type TrayDashboardSuggestionPlacement = 'adjacent-popover' | 'window-side-panel' | 'bottom-dock'

export interface TrayDashboardCapture {
  state: TrayDashboardCaptureState
  capturedCount: number
}

export interface TrayDashboardContext {
  appName: string
  windowTitle: string
  targetLabel: string
}

export interface TrayDashboardProvider {
  id: string
  label: string
  status: TrayDashboardProviderStatus
}

export interface TrayDashboardSuggestion {
  id: string
  title: string
  source: string
  confidence: number
  placement: TrayDashboardSuggestionPlacement
  unread: boolean
  requiresConsent: boolean
}

export interface TrayDashboardPrivacy {
  redaction: TrayDashboardRedactionState
  externalEgress: TrayDashboardExternalEgressState
  auditReady: boolean
}

export interface TrayDashboardProps {
  capture: TrayDashboardCapture
  activeContext: TrayDashboardContext
  providers: TrayDashboardProvider[]
  suggestions: TrayDashboardSuggestion[]
  privacy: TrayDashboardPrivacy
  onCaptureNow: () => void
  onOpenAudit: () => void
  onOpenSettings: () => void
  onOpenSuggestion: (suggestionId: string) => void
  onOpenTimeline: () => void
}

export function TrayDashboard({
  capture,
  activeContext,
  providers,
  suggestions,
  privacy,
  onCaptureNow,
  onOpenAudit,
  onOpenSettings,
  onOpenSuggestion,
  onOpenTimeline,
}: TrayDashboardProps) {
  const { t } = useTranslation()
  const unreadCount = useMemo(() => suggestions.filter((suggestion) => suggestion.unread).length, [suggestions])
  const connectedProviderCount = providers.filter((provider) => provider.status === 'connected').length

  return (
    <section
      aria-label={t('trayDashboard.regionLabel')}
      className="w-[360px] overflow-hidden rounded-lg border border-muted bg-surface-overlay text-content shadow-lg"
    >
      <header className="border-muted border-b bg-surface-elevated px-4 py-3">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="font-medium text-content-strong text-sm">{t('trayDashboard.title')}</p>
            <p className="truncate text-content-secondary text-xs">
              {activeContext.appName} · {activeContext.windowTitle}
            </p>
          </div>
          <StatusPill state={capture.state} />
        </div>
        <div className="mt-3 grid grid-cols-3 gap-2 text-xs">
          <Metric
            icon={<Camera size={14} />}
            label={t('trayDashboard.captureCount', { count: capture.capturedCount })}
          />
          <Metric
            icon={<BellDot size={14} />}
            label={t('trayDashboard.unreadSuggestions', { count: unreadCount })}
            emphasis={unreadCount > 0}
          />
          <Metric
            icon={<Activity size={14} />}
            label={t('trayDashboard.providerCount', { count: connectedProviderCount, total: providers.length })}
          />
        </div>
      </header>

      <div className="space-y-3 px-4 py-3">
        <DashboardSection
          title={t('trayDashboard.currentContext')}
          icon={<Eye size={15} />}
          visualRegion="current-gui-context"
        >
          <div className="rounded-md border border-muted bg-surface-inset p-3">
            <p className="font-medium text-content text-sm">{activeContext.appName}</p>
            <p className="mt-1 truncate text-content-secondary text-xs">{activeContext.targetLabel}</p>
          </div>
        </DashboardSection>

        <DashboardSection
          title={t('trayDashboard.suggestionQueue')}
          icon={<Sparkles size={15} />}
          visualRegion="suggestion-queue"
        >
          <div className="space-y-2">
            {suggestions.map((suggestion) => (
              <button
                key={suggestion.id}
                type="button"
                onClick={() => onOpenSuggestion(suggestion.id)}
                aria-label={t('trayDashboard.reviewSuggestion', { title: suggestion.title })}
                className="w-full rounded-md border border-muted bg-surface-base p-3 text-left transition-colors hover:bg-hover"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <p className="truncate font-medium text-content text-sm">{suggestion.title}</p>
                    <p className="mt-1 truncate text-content-secondary text-xs">{suggestion.source}</p>
                  </div>
                  {suggestion.unread && <span className="mt-1 h-2 w-2 shrink-0 rounded-full bg-brand" />}
                </div>
                <div className="mt-2 flex flex-wrap items-center gap-2 text-[11px]">
                  <Badge>{Math.round(suggestion.confidence * 100)}%</Badge>
                  <Badge>{t(`trayDashboard.placement.${suggestion.placement}`)}</Badge>
                  {suggestion.requiresConsent && <Badge tone="warning">{t('trayDashboard.consentRequired')}</Badge>}
                </div>
              </button>
            ))}
          </div>
        </DashboardSection>

        <DashboardSection title={t('trayDashboard.privacyProvider')} icon={<ShieldCheck size={15} />}>
          <div className="grid grid-cols-2 gap-2">
            <StateTile
              icon={<Lock size={14} />}
              label={redactionLabel(privacy.redaction, t)}
              visualRegion="privacy-redaction-state"
            />
            <StateTile
              icon={<ExternalLink size={14} />}
              label={egressLabel(privacy.externalEgress, t)}
              visualRegion="external-egress-state"
            />
            <StateTile
              icon={<FileClock size={14} />}
              label={privacy.auditReady ? t('trayDashboard.auditReady') : t('trayDashboard.auditPending')}
              visualRegion="audit-affordance"
            />
            <StateTile
              icon={<CheckCircle2 size={14} />}
              label={t('trayDashboard.providersReady', { count: connectedProviderCount })}
              visualRegion="provider-health"
            />
          </div>
          <div className="mt-2 flex flex-wrap gap-1.5" data-visual-region="provider-health">
            {providers.map((provider) => (
              <ProviderBadge key={provider.id} provider={provider} />
            ))}
          </div>
        </DashboardSection>
      </div>

      <footer
        className="grid grid-cols-4 border-muted border-t bg-surface-inset p-2"
        data-visual-region="quick-actions"
      >
        <FooterButton icon={<Camera size={14} />} label={t('trayDashboard.captureNow')} onClick={onCaptureNow} />
        <FooterButton icon={<Archive size={14} />} label={t('trayDashboard.openTimeline')} onClick={onOpenTimeline} />
        <FooterButton icon={<Clock3 size={14} />} label={t('trayDashboard.openAudit')} onClick={onOpenAudit} />
        <FooterButton icon={<Settings size={14} />} label={t('trayDashboard.openSettings')} onClick={onOpenSettings} />
      </footer>
    </section>
  )
}

function StatusPill({ state }: { state: TrayDashboardCaptureState }) {
  const { t } = useTranslation()
  const tone = {
    active: 'bg-semantic-success/15 text-semantic-success',
    paused: 'bg-semantic-warning/15 text-semantic-warning',
    local: 'bg-semantic-info/15 text-semantic-info',
  }[state]

  return (
    <span className={cn('rounded-full px-2.5 py-1 font-medium text-xs', tone)} data-visual-region="capture-state">
      {t(`trayDashboard.captureState.${state}`)}
    </span>
  )
}

function Metric({ icon, label, emphasis }: { icon: React.ReactNode; label: string; emphasis?: boolean }) {
  return (
    <div
      className={cn(
        'flex min-w-0 items-center gap-1.5 rounded-md bg-surface-base px-2 py-1.5',
        emphasis && 'text-brand',
      )}
    >
      <span className="shrink-0">{icon}</span>
      <span className="truncate">{label}</span>
    </div>
  )
}

function DashboardSection({
  title,
  icon,
  children,
  visualRegion,
}: {
  title: string
  icon: React.ReactNode
  children: React.ReactNode
  visualRegion?: string
}) {
  return (
    <section data-visual-region={visualRegion}>
      <h2 className="mb-2 flex items-center gap-1.5 font-medium text-content-strong text-xs">
        {icon}
        <span>{title}</span>
      </h2>
      {children}
    </section>
  )
}

function Badge({ children, tone = 'neutral' }: { children: React.ReactNode; tone?: 'neutral' | 'warning' }) {
  return (
    <span
      className={cn(
        'rounded-full px-2 py-0.5 font-medium',
        tone === 'warning'
          ? 'bg-semantic-warning/15 text-semantic-warning'
          : 'bg-surface-elevated text-content-secondary',
      )}
    >
      {children}
    </span>
  )
}

function StateTile({ icon, label, visualRegion }: { icon: React.ReactNode; label: string; visualRegion?: string }) {
  return (
    <div
      className="flex min-w-0 items-center gap-2 rounded-md bg-surface-inset px-2 py-2 text-content-secondary text-xs"
      data-visual-region={visualRegion}
    >
      <span className="shrink-0 text-content-tertiary">{icon}</span>
      <span className="truncate">{label}</span>
    </div>
  )
}

function ProviderBadge({ provider }: { provider: TrayDashboardProvider }) {
  const { t } = useTranslation()
  const tone = {
    connected: 'bg-semantic-success/15 text-semantic-success',
    unavailable: 'bg-surface-elevated text-content-secondary',
    blocked: 'bg-semantic-warning/15 text-semantic-warning',
  }[provider.status]

  return (
    <span className={cn('rounded-full px-2 py-0.5 text-[11px]', tone)}>
      {provider.label}
      <span className="sr-only"> {t(`trayDashboard.providerStatus.${provider.status}`)}</span>
    </span>
  )
}

function FooterButton({ icon, label, onClick }: { icon: React.ReactNode; label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex min-w-0 flex-col items-center gap-1 rounded-md px-1 py-1.5 text-content-secondary text-xs transition-colors hover:bg-hover hover:text-content"
    >
      {icon}
      <span className="max-w-full truncate">{label}</span>
    </button>
  )
}

function redactionLabel(redaction: TrayDashboardRedactionState, t: (key: string) => string) {
  return t(`trayDashboard.redaction.${redaction}`)
}

function egressLabel(egress: TrayDashboardExternalEgressState, t: (key: string) => string) {
  return t(`trayDashboard.egress.${egress}`)
}
