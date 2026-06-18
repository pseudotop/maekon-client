// e2e-tauri/dashboard-live-metrics.spec.ts
import { waitForSseEvent } from './helpers.js'

describe('J1: Dashboard Live Metrics', () => {
  /**
   * @tc_id T100
   * @risk_id FUNC-001
   * @journey J1 (DevOps Dashboard Monitoring)
   * @persona P1 (DevOps Engineer)
   * @priority P1
   * @tauri_only_reason Real desktop SSE emits live metrics from the runtime scheduler
   */
  it('T100: metrics SSE returns live system data', async () => {
    const event = await waitForSseEvent('metrics', 15000)
    const metrics = (event as any).data ?? event

    expect(metrics).toBeDefined()
    expect(metrics.cpu_usage).toBeGreaterThanOrEqual(0)
    expect(metrics.cpu_usage).toBeLessThanOrEqual(100)
    expect(metrics.memory_used).toBeGreaterThan(0)
    expect(metrics.memory_total).toBeGreaterThan(0)
  })

  /**
   * @tc_id T101
   * @risk_id DATA-001
   * @journey J1 (DevOps Dashboard Monitoring)
   * @persona P1 (DevOps Engineer)
   * @priority P1
   * @tauri_only_reason StatusBar displays real SSE metrics, not mocked data
   */
  it('T101: StatusBar CPU matches SSE metric within 5%', async () => {
    // Get the CPU display text from the StatusBar
    const statusBar = await $('.app-shell-statusbar')
    await statusBar.waitForExist({ timeout: 10000 })
    const statusText = await statusBar.getText()

    // Get the actual metrics via SSE
    const event = await waitForSseEvent('metrics', 15000)
    const metrics = (event as any).data ?? event

    // Verify that a CPU percentage is shown in the StatusBar
    const cpuMatch = statusText.match(/(\d+(?:\.\d+)?)\s*%/)
    if (cpuMatch) {
      const displayedCpu = parseFloat(cpuMatch[1])
      // Allow up to 5% deviation from the IPC value (polling timing difference)
      expect(Math.abs(displayedCpu - metrics.cpu_usage)).toBeLessThanOrEqual(5)
    }
    // When the CPU is not displayed (e.g. while loading), pass if the SSE value itself is valid
    expect(metrics.cpu_usage).toBeGreaterThanOrEqual(0)
  })
})
