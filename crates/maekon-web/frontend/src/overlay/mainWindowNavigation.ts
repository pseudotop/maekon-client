/** Restore the primary Tauri window before routing its React tree. */
export async function navigateMainWindow(route: string): Promise<void> {
  const [{ invoke }, { emit }] = await Promise.all([import('@tauri-apps/api/core'), import('@tauri-apps/api/event')])
  await invoke('show_main_window')
  await emit('navigate', route)
}
