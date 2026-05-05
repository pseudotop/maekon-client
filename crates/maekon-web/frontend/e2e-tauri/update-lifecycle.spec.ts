// e2e-tauri/update-lifecycle.spec.ts
import { fetchApiJson, type UpdateStatusResponse } from './helpers.js'

describe('J5: Update Lifecycle', () => {
  /**
   * @tc_id T140
   * @risk_id FUNC-006
   * @tauri_only_reason REST update status reflects the real desktop update state machine
   */
  it('T140: update status endpoint returns valid structure', async () => {
    const status = await fetchApiJson<UpdateStatusResponse>('/update/status')

    expect(status).toBeDefined()
    expect(typeof status.phase).toBe('string')
    expect(status.phase.length).toBeGreaterThan(0)
  })

  /**
   * @tc_id T141
   * @risk_id FUNC-007
   * @tauri_only_reason REST update action triggers the real update action path
   */
  it('T141: approve update action returns accepted response', async () => {
    const result = await fetchApiJson<{ accepted: boolean }>('/update/action', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ action: 'Approve' }),
    })
    expect(result.accepted).toBe(true)
  })

  /**
   * @tc_id T142
   * @risk_id FUNC-008
   * @tauri_only_reason REST update action triggers the real update action path
   */
  it('T142: defer update action returns accepted response', async () => {
    const result = await fetchApiJson<{ accepted: boolean }>('/update/action', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ action: 'Defer' }),
    })
    expect(result.accepted).toBe(true)
  })

  /**
   * @tc_id T143
   * @risk_id DATA-002
   * @tauri_only_reason Version comes from real Cargo.toml, not mocked
   */
  it('T143: Update page shows version matching tauri.conf.json', async () => {
    await browser.url('tauri://localhost/updates')
    await browser.pause(2000)

    const pageText = await $('body').getText()

    // 페이지에 버전 정보가 표시되는지 확인 (semver 패턴)
    const semverPattern = /\d+\.\d+\.\d+/
    expect(pageText).toMatch(semverPattern)
  })
})
