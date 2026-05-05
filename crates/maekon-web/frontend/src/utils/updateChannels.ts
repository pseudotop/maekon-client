import type { UpdateChannel } from '../api/client'

export const NIGHTLY_UPDATE_CHANNEL_AVAILABLE = false

export function isSelectableUpdateChannel(channel: UpdateChannel): boolean {
  return channel !== 'nightly' || NIGHTLY_UPDATE_CHANNEL_AVAILABLE
}
