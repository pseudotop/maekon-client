import { Clock, RefreshCw, RotateCcw, Shield } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { UpdateChannel } from '../../api/client'
import { Alert, Badge, Card, CardTitle, Spinner } from '../../components/ui'
import { useTypedOutletContext } from '../../routes'
import { interaction, motion, typography } from '../../styles/tokens'
import { cn } from '../../utils/cn'
import { isSelectableUpdateChannel } from '../../utils/updateChannels'
import type { UpdatesOutletContext } from './UpdatesLayout'

const FEATURE_ICONS = [RefreshCw, Shield, RotateCcw, Clock] as const

const CHANNEL_OPTIONS: { value: UpdateChannel; labelKey: string; descKey: string }[] = [
  {
    value: 'stable',
    labelKey: 'settings.channelStable',
    descKey: 'updates.channelStableDesc',
  },
  {
    value: 'pre_release',
    labelKey: 'settings.channelPreRelease',
    descKey: 'updates.channelPreReleaseDesc',
  },
  {
    value: 'nightly',
    labelKey: 'settings.channelNightly',
    descKey: 'updates.channelNightlyDesc',
  },
]

export default function ChannelSection() {
  const { t } = useTranslation()
  const { currentChannel, savingChannel, handleChannelChange } = useTypedOutletContext<UpdatesOutletContext>('Updates')
  const currentChannelUnavailable = !isSelectableUpdateChannel(currentChannel)

  const featureKeys = [
    'updates.featureAuto',
    'updates.featureIntegrity',
    'updates.featureRollback',
    'updates.featureMinimal',
  ] as const

  return (
    <>
      {/* Channel selector */}
      <Card id="section-channel" variant="default" padding="lg">
        <CardTitle className="mb-4">{t('updates.channelTitle', 'Update Channel')}</CardTitle>
        {currentChannelUnavailable && (
          <Alert variant="warning" title={t('updates.channelNightlyDisabledTitle')} className="mb-4">
            {t('updates.channelNightlyDisabledDescription')}
          </Alert>
        )}
        <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
          {CHANNEL_OPTIONS.map((opt) => {
            const isUnavailable = !isSelectableUpdateChannel(opt.value)
            const isCurrent = currentChannel === opt.value
            const isActive = isCurrent && !isUnavailable
            return (
              <button
                key={opt.value}
                type="button"
                disabled={savingChannel || isUnavailable}
                onClick={() => {
                  if (!isUnavailable) handleChannelChange(opt.value)
                }}
                className={cn(
                  'rounded-lg border p-3 text-left',
                  motion.colors,
                  interaction.disabled,
                  isActive
                    ? 'border-brand-text bg-brand-muted/10'
                    : isUnavailable
                      ? 'border-muted bg-surface-muted/70 text-content-tertiary'
                      : 'border-muted bg-surface hover:border-brand-muted hover:bg-surface-muted',
                )}
              >
                <div className="flex items-center justify-between">
                  <span
                    className={cn(
                      'text-sm',
                      typography.weight.medium,
                      isActive ? 'text-brand-text' : 'text-content-strong',
                      isUnavailable && 'text-content-secondary',
                    )}
                  >
                    {t(opt.labelKey, opt.value)}
                  </span>
                  {isActive && (
                    <Badge color="success" size="sm">
                      {t('updates.active', 'Active')}
                    </Badge>
                  )}
                  {isUnavailable && (
                    <Badge color={isCurrent ? 'warning' : 'default'} size="sm">
                      {isCurrent ? t('updates.needsChange') : t('updates.notAvailable')}
                    </Badge>
                  )}
                </div>
                <p className="mt-1 text-content-secondary text-xs">
                  {isUnavailable ? t('updates.channelNightlyDisabledCard') : t(opt.descKey, '')}
                </p>
              </button>
            )
          })}
        </div>
        {savingChannel && (
          <div className="mt-3 flex items-center gap-2 text-content-secondary text-sm">
            <Spinner size="sm" />
            {t('updates.savingChannel', 'Saving...')}
          </div>
        )}
      </Card>

      {/* Features */}
      <Card id="section-features" variant="default" padding="lg">
        <CardTitle className="mb-4">{t('updates.featuresTitle')}</CardTitle>
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          {featureKeys.map((key, index) => {
            const Icon = FEATURE_ICONS[index]

            return (
              <div key={key} className="flex items-start gap-3 rounded-lg bg-surface-muted p-3">
                <div className="mt-0.5 shrink-0 text-brand-text">
                  <Icon size={18} aria-hidden="true" />
                </div>
                <span className="text-content-strong text-sm">{t(key)}</span>
              </div>
            )
          })}
        </div>
      </Card>

      {/* Security policy */}
      <Card id="section-policy" variant="default" padding="lg">
        <CardTitle className="mb-3">{t('updates.policyTitle')}</CardTitle>
        <ul className="space-y-1 text-content-strong text-sm">
          <li>{t('updates.policyIntegrity')}</li>
          <li>{t('updates.policySignature')}</li>
          <li>{t('updates.policyRollback')}</li>
        </ul>
      </Card>
    </>
  )
}
