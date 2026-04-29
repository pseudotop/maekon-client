import { useTranslation } from 'react-i18next'
import { changeLanguage, getCurrentLanguage, type SupportedLanguageCode, supportedLanguages } from '../i18n'
import { Select } from './ui'

export default function LanguageSelector() {
  const { t } = useTranslation()
  const currentLang = getCurrentLanguage()

  const handleLanguageChange = (value: string) => {
    changeLanguage(value as SupportedLanguageCode)
  }

  return (
    <div className="max-w-xs">
      <Select
        aria-label={t('settings.language')}
        value={currentLang}
        onChange={(event) => handleLanguageChange(event.target.value)}
      >
        {supportedLanguages.map((lang) => (
          <option key={lang.code} value={lang.code}>
            {lang.name}
          </option>
        ))}
      </Select>
    </div>
  )
}
