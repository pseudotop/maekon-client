import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Badge, Button, FieldHint, Input, Select } from '../../../components/ui'
import { form, typography } from '../../../styles/tokens'
import type { ProfileManagerProps } from './types'

export default function ProfileManager({
  formData,
  onSelectAiProviderProfile,
  onSaveAiProviderProfile,
  onDeleteAiProviderProfile,
}: ProfileManagerProps) {
  const { t } = useTranslation()
  const savedProfiles = formData.ai_provider.saved_profiles ?? []
  const activeSavedProfile =
    savedProfiles.find((profile) => profile.profile_id === formData.ai_provider.active_profile_id) ?? null
  const [profileNameDraft, setProfileNameDraft] = useState('')

  useEffect(() => {
    setProfileNameDraft(activeSavedProfile?.name ?? '')
  }, [activeSavedProfile?.name])

  const handleSavedProfileSelection = (profileId: string) => {
    const nextProfile = savedProfiles.find((profile) => profile.profile_id === profileId) ?? null
    if (nextProfile) {
      setProfileNameDraft(nextProfile.name)
    }
    onSelectAiProviderProfile(profileId || null)
  }

  const handleSaveCurrentProfile = () => {
    const nextName = profileNameDraft.trim() || activeSavedProfile?.name || ''
    if (!nextName) {
      return
    }
    setProfileNameDraft(nextName)
    onSaveAiProviderProfile(nextName)
  }

  const handleDeleteCurrentProfile = () => {
    if (!activeSavedProfile) {
      return
    }
    setProfileNameDraft('')
    onDeleteAiProviderProfile(activeSavedProfile.profile_id)
  }

  const saveProfileDisabled = !profileNameDraft.trim() && !activeSavedProfile?.name

  return (
    <div className="space-y-3 rounded-lg border border-muted p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <p className={`${typography.weight.medium} text-content-strong text-sm`}>
            {t('settingsAutomation.savedProfilesTitle')}
          </p>
          <p className="text-content-secondary text-sm">{t('settingsAutomation.savedProfilesDescription')}</p>
        </div>
        {activeSavedProfile ? (
          <Badge color="info" size="sm">
            {t('settingsAutomation.savedProfilesActiveBadge')}
          </Badge>
        ) : null}
      </div>

      <div>
        <label htmlFor="settings-ai-provider-profile" className={form.label}>
          {t('settingsAutomation.savedProfilesSelectLabel')}
        </label>
        <Select
          id="settings-ai-provider-profile"
          value={formData.ai_provider.active_profile_id ?? ''}
          onChange={(e) => handleSavedProfileSelection(e.target.value)}
        >
          <option value="">{t('settingsAutomation.savedProfilesCustomOption')}</option>
          {savedProfiles.map((profile) => (
            <option key={profile.profile_id} value={profile.profile_id}>
              {profile.name}
            </option>
          ))}
        </Select>
        <FieldHint>{t('settingsAutomation.savedProfilesSelectHint')}</FieldHint>
      </div>

      <div className="grid grid-cols-1 gap-3 md:grid-cols-[minmax(0,1fr)_auto_auto]">
        <div>
          <label htmlFor="settings-ai-provider-profile-name" className={form.label}>
            {t('settingsAutomation.savedProfilesNameLabel')}
          </label>
          <Input
            id="settings-ai-provider-profile-name"
            type="text"
            value={profileNameDraft}
            onChange={(e) => setProfileNameDraft(e.target.value)}
            placeholder={t('settingsAutomation.savedProfilesNamePlaceholder')}
          />
          <FieldHint>{t('settingsAutomation.savedProfilesNameHint')}</FieldHint>
        </div>
        <div className="flex items-end">
          <Button type="button" variant="secondary" onClick={handleSaveCurrentProfile} disabled={saveProfileDisabled}>
            {activeSavedProfile
              ? t('settingsAutomation.savedProfilesUpdateAction')
              : t('settingsAutomation.savedProfilesSaveAction')}
          </Button>
        </div>
        <div className="flex items-end">
          <Button type="button" variant="danger" onClick={handleDeleteCurrentProfile} disabled={!activeSavedProfile}>
            {t('settingsAutomation.savedProfilesDeleteAction')}
          </Button>
        </div>
      </div>

      <p className="text-content-secondary text-xs">
        {activeSavedProfile
          ? t('settingsAutomation.savedProfilesSelectedHint', { name: activeSavedProfile.name })
          : t('settingsAutomation.savedProfilesCustomHint')}
      </p>
    </div>
  )
}
