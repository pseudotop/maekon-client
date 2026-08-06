import { useTranslation } from 'react-i18next'
import { Card, CardTitle, FieldHint, Select } from '../../../components/ui'
import { form } from '../../../styles/tokens'
import { IS_WINDOWS } from '../../../utils/platform'
import ToggleRow from '../ToggleRow'
import type { SandboxConfigProps } from './types'

export default function SandboxConfig({ formData, onSandboxChange }: SandboxConfigProps) {
  const { t } = useTranslation()

  return (
    <Card variant="default" padding="lg">
      <CardTitle sticky>{t('settingsAutomation.sandboxTitle')}</CardTitle>
      <div className="space-y-4">
        <ToggleRow
          label={t('settingsAutomation.sandboxEnabled')}
          description={t('settingsAutomation.sandboxEnabledDescription')}
          checked={formData.sandbox.enabled}
          onChange={(value) => onSandboxChange('enabled', value)}
        />

        <div className={`space-y-4 ${!formData.sandbox.enabled ? 'pointer-events-none opacity-50' : ''}`}>
          <div>
            <label htmlFor="settings-sandbox-profile" className={form.label}>
              {t('settingsAutomation.sandboxProfile')}
            </label>
            <Select
              id="settings-sandbox-profile"
              value={formData.sandbox.profile}
              onChange={(e) => onSandboxChange('profile', e.target.value)}
            >
              <option value="Permissive">{t('settingsAutomation.sandboxProfilePermissive')}</option>
              <option value="Standard" disabled={IS_WINDOWS}>
                {t('settingsAutomation.sandboxProfileStandard')}
                {IS_WINDOWS ? ` — ${t('settingsAutomation.sandboxProfileUnavailable')}` : ''}
              </option>
              <option value="Strict" disabled={IS_WINDOWS}>
                {t('settingsAutomation.sandboxProfileStrict')}
                {IS_WINDOWS ? ` — ${t('settingsAutomation.sandboxProfileUnavailable')}` : ''}
              </option>
            </Select>
            <FieldHint>
              {t(IS_WINDOWS ? 'settingsAutomation.sandboxProfileWindowsHint' : 'settingsAutomation.sandboxProfileHint')}
            </FieldHint>
          </div>

          <ToggleRow
            label={t('settingsAutomation.allowNetwork')}
            description={t('settingsAutomation.allowNetworkDescription')}
            checked={formData.sandbox.allow_network}
            onChange={(value) => onSandboxChange('allow_network', value)}
          />
        </div>
      </div>
    </Card>
  )
}
