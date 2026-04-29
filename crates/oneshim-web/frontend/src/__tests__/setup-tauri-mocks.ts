import { clearMocks, mockIPC, mockWindows } from '@tauri-apps/api/mocks'

export function setupTauriMocks() {
  beforeEach(() => {
    mockWindows('main')
  })

  afterEach(() => {
    clearMocks()
  })
}

export function mockCommand(command: string, handler: (args: Record<string, unknown>) => unknown) {
  mockIPC((cmd, args) => {
    if (cmd === command) return handler(args as Record<string, unknown>)
    return undefined
  })
}
