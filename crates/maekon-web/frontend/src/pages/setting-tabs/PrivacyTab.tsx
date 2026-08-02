import { useSettingsFormContext } from '../settings/SettingsFormContext'
import CaptureReauthSettings from './CaptureReauthSettings'
import MemoryVaultSettings from './MemoryVaultSettings'
import PrivacySettings from './PrivacySettings'

export default function PrivacyTab() {
  const { form } = useSettingsFormContext()
  if (!form.formData) return null

  return (
    <div id="section-privacy" className="space-y-6">
      <PrivacySettings privacy={form.formData.privacy} onChange={form.handlePrivacyChange} />
      {/* #8044: capture-history re-authentication (biometric/PIN) — managed via dedicated IPC, separate from the config form. */}
      <CaptureReauthSettings />
      {/* ADR-033 (#9465): memory vault mirror — data-ownership surface. Also
          dedicated IPC: the Tier-13 consent grant and the §3.3 custom-path
          acknowledgement are gated writes, not config-form fields. */}
      <MemoryVaultSettings />
    </div>
  )
}
