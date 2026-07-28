import { useSettingsFormContext } from '../settings/SettingsFormContext'
import CaptureReauthSettings from './CaptureReauthSettings'
import PrivacySettings from './PrivacySettings'

export default function PrivacyTab() {
  const { form } = useSettingsFormContext()
  if (!form.formData) return null

  return (
    <div id="section-privacy" className="space-y-6">
      <PrivacySettings privacy={form.formData.privacy} onChange={form.handlePrivacyChange} />
      {/* #8044: capture-history re-authentication (biometric/PIN) — managed via dedicated IPC, separate from the config form. */}
      <CaptureReauthSettings />
    </div>
  )
}
