/**
 *
 */
import { useTranslation } from 'react-i18next'
import type { PrivacySettings as PrivacySettingsType } from '../../api/client'
import { Card, CardTitle, GuidancePanel, Input, Select } from '../../components/ui'
import { form } from '../../styles/tokens'
import ToggleRow from './ToggleRow'

interface PrivacySettingsProps {
  privacy: PrivacySettingsType
  onChange: (field: keyof PrivacySettingsType, value: boolean | string | string[]) => void
}

export default function PrivacySettings({ privacy, onChange }: PrivacySettingsProps) {
  const { t } = useTranslation()

  return (
    <div className="space-y-6">
      <GuidancePanel
        title={t('settings.guidance.privacy.title')}
        description={t('settings.guidance.privacy.description')}
        items={[
          {
            title: t('settings.guidance.privacy.automatic.title'),
            description: t('settings.guidance.privacy.automatic.description'),
          },
          {
            title: t('settings.guidance.privacy.pii.title'),
            description: t('settings.guidance.privacy.pii.description'),
          },
          {
            title: t('settings.guidance.privacy.patterns.title'),
            description: t('settings.guidance.privacy.patterns.description'),
          },
        ]}
      />

      <Card variant="default" padding="lg">
        <CardTitle sticky>{t('settings.privacyTitle')}</CardTitle>
        <div className="space-y-4">
          <ToggleRow
            label={t('settings.autoExclude')}
            description={t('settings.autoExcludeDesc')}
            checked={privacy.auto_exclude_sensitive}
            onChange={(v) => onChange('auto_exclude_sensitive', v)}
          />

          <div>
            <label htmlFor="privacy-pii-level" className={form.label}>
              {t('settings.piiLevel')}
            </label>
            <Select
              id="privacy-pii-level"
              value={privacy.pii_filter_level}
              onChange={(e) => onChange('pii_filter_level', e.target.value)}
            >
              <option value="Off">{t('settings.piiOff')}</option>
              <option value="Basic">{t('settings.piiBasic')}</option>
              <option value="Standard">{t('settings.piiStandard')}</option>
              <option value="Strict">{t('settings.piiStrict')}</option>
            </Select>
            <p className={form.helper}>{t('settings.piiDesc')}</p>
          </div>

          <div>
            <label htmlFor="privacy-excluded-apps" className={form.label}>
              {t('settings.excludedApps')}
            </label>
            <Input
              id="privacy-excluded-apps"
              type="text"
              value={privacy.excluded_apps.join(', ')}
              onChange={(e) =>
                onChange(
                  'excluded_apps',
                  e.target.value
                    .split(',')
                    .map((s) => s.trim())
                    .filter(Boolean),
                )
              }
              placeholder={t('settings.excludedAppsPlaceholder', '1Password, Discord, Slack')}
            />
          </div>

          <div>
            <label htmlFor="privacy-excluded-app-patterns" className={form.label}>
              {t('settings.excludedAppPatterns')}
            </label>
            <Input
              id="privacy-excluded-app-patterns"
              type="text"
              value={privacy.excluded_app_patterns.join(', ')}
              onChange={(e) =>
                onChange(
                  'excluded_app_patterns',
                  e.target.value
                    .split(',')
                    .map((s) => s.trim())
                    .filter(Boolean),
                )
              }
              placeholder={t('settings.excludedAppPatternsPlaceholder', '*bank*, *wallet*, *crypto*')}
            />
            <p className={form.helper}>{t('settings.wildcardHint')}</p>
          </div>

          <div>
            <label htmlFor="privacy-excluded-title-patterns" className={form.label}>
              {t('settings.excludedTitlePatterns')}
            </label>
            <Input
              id="privacy-excluded-title-patterns"
              type="text"
              value={privacy.excluded_title_patterns.join(', ')}
              onChange={(e) =>
                onChange(
                  'excluded_title_patterns',
                  e.target.value
                    .split(',')
                    .map((s) => s.trim())
                    .filter(Boolean),
                )
              }
              placeholder={t('settings.excludedTitlePatternsPlaceholder', '*password*, *secret*, *private*')}
            />
          </div>
        </div>
      </Card>
    </div>
  )
}
