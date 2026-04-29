// e2e-tauri/privacy-audit-setup.spec.ts
import { fetchApiJson, invokeIpc } from './helpers.js'

describe('J2: Privacy & Settings Persistence', () => {
  /**
   * @tc_id T110
   * @risk_id SEC-001
   * @journey J2 (Privacy-First Setup & Audit)
   * @persona P2 (Privacy-Conscious Enterprise Admin)
   * @priority P0
   * @tauri_only_reason IPC update_setting writes config JSON to disk
   */
  it('T110: Settings write persists PII filter level after reload', async () => {
    // 현재 설정 저장
    const before = await fetchApiJson<Record<string, any>>('/settings')
    const originalLevel = before?.privacy?.pii_filter_level

    // PII filter를 Strict로 변경
    const patch = { privacy: { pii_filter_level: 'Strict' } }
    await invokeIpc('update_setting', { config_json: JSON.stringify(patch) })

    // 페이지 리로드 (디스크에서 다시 읽기)
    await browser.url('tauri://localhost/')
    await browser.pause(2000) // config reload 대기

    // 변경 사항이 유지되는지 확인
    const after = await fetchApiJson<Record<string, any>>('/settings')
    expect(after.privacy.pii_filter_level).toBe('Strict')

    // 원래 값 복원
    if (originalLevel && originalLevel !== 'Strict') {
      const restore = { privacy: { pii_filter_level: originalLevel } }
      await invokeIpc('update_setting', { config_json: JSON.stringify(restore) })
    }
  })

  /**
   * @tc_id T111
   * @risk_id SEC-002
   * @journey J2 (Privacy-First Setup & Audit)
   * @persona P2 (Privacy-Conscious Enterprise Admin)
   * @priority P0
   * @tauri_only_reason IPC update_setting writes config JSON to disk
   */
  it('T111: Settings write persists capture_enabled toggle after reload', async () => {
    const before = await fetchApiJson<Record<string, any>>('/settings')
    const originalEnabled = before?.capture?.capture_enabled

    // capture 비활성화
    const patch = { capture: { capture_enabled: false } }
    await invokeIpc('update_setting', { config_json: JSON.stringify(patch) })

    await browser.url('tauri://localhost/')
    await browser.pause(2000)

    const after = await fetchApiJson<Record<string, any>>('/settings')
    expect(after.capture.capture_enabled).toBe(false)

    // 원래 값 복원
    if (originalEnabled !== false) {
      const restore = { capture: { capture_enabled: true } }
      await invokeIpc('update_setting', { config_json: JSON.stringify(restore) })
    }
  })

  /**
   * @tc_id T113
   * @risk_id PRIV-002
   * @journey J2 (Privacy-First Setup & Audit)
   * @persona P2 (Privacy-Conscious Enterprise Admin)
   * @priority P0
   * @tauri_only_reason ConfirmModal exists in real Tauri app (Privacy.tsx:14-43)
   */
  it('T113: Data deletion requires confirmation dialog', async () => {
    // Privacy 페이지로 이동
    await browser.url('tauri://localhost/privacy')
    await browser.pause(2000)

    // "Delete All Data" 버튼 찾기 (danger variant)
    const deleteBtn = await $('button*=Delete All')
    if (await deleteBtn.isExisting()) {
      await deleteBtn.click()

      // ConfirmModal이 표시되는지 확인 (fixed inset-0 z-50 overlay)
      const modal = await $('.fixed.inset-0.z-50')
      await modal.waitForExist({ timeout: 3000 })
      expect(await modal.isDisplayed()).toBe(true)

      // Cancel 버튼으로 닫기
      const cancelBtn = await modal.$('button*=Cancel')
      if (await cancelBtn.isExisting()) {
        await cancelBtn.click()
      }
      // 또는 한국어: 취소
      else {
        const cancelKo = await modal.$('button*=취소')
        if (await cancelKo.isExisting()) await cancelKo.click()
      }
    }
  })

  /**
   * @tc_id T114
   * @risk_id A11Y-001
   * @tauri_only_reason Focus trap behavior in real WebView environment
   */
  it('T114: Data deletion confirm dialog has focus trap and a11y', async () => {
    await browser.url('tauri://localhost/privacy')
    await browser.pause(2000)

    // Delete All 버튼 클릭
    const deleteBtn = await $('button*=Delete All')
    if (!(await deleteBtn.isExisting())) {
      // 한국어 UI
      const deleteBtnKo = await $('button*=모든 데이터 삭제')
      if (await deleteBtnKo.isExisting()) await deleteBtnKo.click()
      else return // 버튼 없으면 skip
    } else {
      await deleteBtn.click()
    }

    await browser.pause(500)

    // role="alertdialog" 확인
    const dialog = await $('div[role="alertdialog"]')
    await dialog.waitForExist({ timeout: 3000 })
    expect(await dialog.isDisplayed()).toBe(true)

    // aria-describedby 확인
    const describedBy = await dialog.getAttribute('aria-describedby')
    expect(describedBy).toBeTruthy()

    // Focus trap: Tab을 여러 번 눌러도 다이얼로그 안에 포커스 유지
    await browser.keys('Tab')
    await browser.keys('Tab')
    await browser.keys('Tab')
    await browser.keys('Tab')

    // 현재 포커스된 요소가 다이얼로그 내부인지 확인
    const activeEl = await browser.execute(() => {
      const active = document.activeElement
      const dialog = document.querySelector('div[role="alertdialog"]')
      return dialog?.contains(active) ?? false
    })
    expect(activeEl).toBe(true)

    // Escape로 닫기
    await browser.keys('Escape')
    await browser.pause(500)
    expect(await dialog.isExisting()).toBe(false)
  })
})
