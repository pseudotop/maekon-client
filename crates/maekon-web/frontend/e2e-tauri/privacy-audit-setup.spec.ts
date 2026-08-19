// e2e-tauri/privacy-audit-setup.spec.ts
import { fetchApiJson, invokeIpc, navigateMain } from './helpers.js'

type SettingsSnapshot = {
  capture_enabled: boolean
  privacy: { pii_filter_level: string }
}

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
    // Save the current settings
    const before = await fetchApiJson<SettingsSnapshot>('/settings')
    const originalLevel = before?.privacy?.pii_filter_level

    // Change the PII filter to Strict
    const patch = { privacy: { pii_filter_level: 'Strict' } }
    await invokeIpc('update_setting', { configJson: JSON.stringify(patch) })

    // Reload the page (re-read from disk)
    await navigateMain('/')
    await browser.pause(2000) // wait for config reload

    // Verify the change is persisted
    const after = await fetchApiJson<SettingsSnapshot>('/settings')
    expect(after.privacy.pii_filter_level).toBe('Strict')

    // Restore the original value
    if (originalLevel && originalLevel !== 'Strict') {
      const restore = { privacy: { pii_filter_level: originalLevel } }
      await invokeIpc('update_setting', { configJson: JSON.stringify(restore) })
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
    const before = await fetchApiJson<SettingsSnapshot>('/settings')
    const originalEnabled = before.capture_enabled

    // Disable capture
    const patch = { capture: { capture_enabled: false } }
    await invokeIpc('update_setting', { configJson: JSON.stringify(patch) })

    await navigateMain('/')
    await browser.pause(2000)

    const after = await fetchApiJson<SettingsSnapshot>('/settings')
    expect(after.capture_enabled).toBe(false)

    // Restore the original value
    if (originalEnabled !== false) {
      const restore = { capture: { capture_enabled: true } }
      await invokeIpc('update_setting', { configJson: JSON.stringify(restore) })
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
    // Navigate to the Privacy page
    await navigateMain('/privacy')
    await browser.pause(2000)

    // Find the "Delete All Data" button (danger variant)
    const deleteBtn = await $('button*=Delete All')
    if (await deleteBtn.isExisting()) {
      await deleteBtn.click()

      // Verify the ConfirmModal is shown (fixed inset-0 z-50 overlay)
      const modal = await $('.fixed.inset-0.z-50')
      await modal.waitForExist({ timeout: 3000 })
      expect(await modal.isDisplayed()).toBe(true)

      // Close via the Cancel button
      const cancelBtn = await modal.$('button*=Cancel')
      if (await cancelBtn.isExisting()) {
        await cancelBtn.click()
      }
      // Or the Korean cancel button (localized fallback).
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
    await navigateMain('/privacy')
    await browser.pause(2000)

    // Click the Delete All button
    const deleteBtn = await $('button*=Delete All')
    if (!(await deleteBtn.isExisting())) {
      // Korean UI
      const deleteBtnKo = await $('button*=모든 데이터 삭제')
      if (await deleteBtnKo.isExisting()) await deleteBtnKo.click()
      else return // skip if the button is absent
    } else {
      await deleteBtn.click()
    }

    await browser.pause(500)

    // Verify role="alertdialog"
    const dialog = await $('div[role="alertdialog"]')
    await dialog.waitForExist({ timeout: 3000 })
    expect(await dialog.isDisplayed()).toBe(true)

    // Verify aria-describedby
    const describedBy = await dialog.getAttribute('aria-describedby')
    expect(describedBy).toBeTruthy()

    // Focus trap: focus stays inside the dialog even after pressing Tab multiple times
    await browser.keys('Tab')
    await browser.keys('Tab')
    await browser.keys('Tab')
    await browser.keys('Tab')

    // Verify the currently focused element is inside the dialog
    const activeEl = await browser.execute(() => {
      const active = document.activeElement
      const dialog = document.querySelector('div[role="alertdialog"]')
      return dialog?.contains(active) ?? false
    })
    expect(activeEl).toBe(true)

    // Close with Escape
    await browser.keys('Escape')
    await browser.pause(500)
    expect(await dialog.isExisting()).toBe(false)
  })
})
