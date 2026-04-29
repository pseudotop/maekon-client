import { isStandaloneModeEnabled } from '../../api/standalone'
import { IS_TAURI } from '../../utils/platform'

export function isProviderOAuthAccessMode(value: string | null | undefined): boolean {
  const normalized = (value ?? '').trim().toLowerCase()
  return normalized === 'provider_oauth' || normalized === 'provideroauth'
}

export function isOAuthPanelAvailableForRuntime(options: { isStandaloneMode: boolean; isTauri: boolean }): boolean {
  return options.isTauri && !options.isStandaloneMode
}

export function isOAuthPanelAvailable(): boolean {
  return isOAuthPanelAvailableForRuntime({
    isStandaloneMode: isStandaloneModeEnabled(),
    isTauri: IS_TAURI,
  })
}
