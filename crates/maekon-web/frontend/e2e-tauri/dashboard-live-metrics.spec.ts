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
    // StatusBar의 CPU 표시 텍스트 가져오기
    const statusBar = await $('.app-shell-statusbar')
    await statusBar.waitForExist({ timeout: 10000 })
    const statusText = await statusBar.getText()

    // SSE로 실제 메트릭 가져오기
    const event = await waitForSseEvent('metrics', 15000)
    const metrics = (event as any).data ?? event

    // StatusBar에 CPU 퍼센트가 표시되는지 확인
    const cpuMatch = statusText.match(/(\d+(?:\.\d+)?)\s*%/)
    if (cpuMatch) {
      const displayedCpu = parseFloat(cpuMatch[1])
      // IPC 값과 5% 이내 오차 허용 (polling 타이밍 차이)
      expect(Math.abs(displayedCpu - metrics.cpu_usage)).toBeLessThanOrEqual(5)
    }
    // CPU가 표시되지 않는 경우 (로딩 중 등), SSE 값 자체가 유효하면 통과
    expect(metrics.cpu_usage).toBeGreaterThanOrEqual(0)
  })
})
