import { useTranslation } from 'react-i18next'
import { Card, CardTitle, FieldHint, Select } from '../../../components/ui'
import { form } from '../../../styles/tokens'
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
              <option value="Standard">{t('settingsAutomation.sandboxProfileStandard')}</option>
              <option value="Strict">{t('settingsAutomation.sandboxProfileStrict')}</option>
            </Select>
            <FieldHint>{t('settingsAutomation.sandboxProfileHint')}</FieldHint>
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
