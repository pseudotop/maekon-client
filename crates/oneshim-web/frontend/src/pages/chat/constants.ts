export const STATE_DOT: Record<string, string> = {
  active: 'bg-status-connected',
  idle: 'bg-status-connecting',
  starting: 'bg-status-connecting',
  recovering: 'bg-semantic-warning',
  failed: 'bg-status-error',
  terminated: 'bg-status-disconnected',
}

export const MAX_CACHED_SESSIONS = 20
