import { useSettingsFormContext } from '../settings/SettingsFormContext'
import PrivacySettings from './PrivacySettings'

export default function PrivacyTab() {
  const { form } = useSettingsFormContext()
  if (!form.formData) return null

  return (
    <div id="section-privacy">
      <PrivacySettings privacy={form.formData.privacy} onChange={form.handlePrivacyChange} />
    </div>
  )
}
