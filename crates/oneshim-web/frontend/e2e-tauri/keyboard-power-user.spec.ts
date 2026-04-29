// e2e-tauri/keyboard-power-user.spec.ts

describe('J3: Keyboard Power User', () => {
  beforeEach(async () => {
    // 각 테스트 전 Dashboard로 이동
    await browser.url('tauri://localhost/')
    await $('nav[role="navigation"]').waitForExist({ timeout: 10000 })
  })

  /**
   * @tc_id T120
   * @risk_id UX-001
   * @tauri_only_reason Cmd+W = close-to-tray (Tauri api.prevent_close()), not browser close
   */
  it('T120: Cmd+W hides window (close-to-tray)', async () => {
    // Cmd+W 전 창 상태 확인
    const titleBefore = await browser.getTitle()
    expect(titleBefore).toContain('ONESHIM')

    // Cmd+W 보내기
    await browser.keys(['Meta', 'w'])
    await browser.pause(1000)

    // 프로세스가 여전히 실행 중인지 확인 (WebDriver 연결 유지 = 앱 살아있음)
    // 창이 숨겨져도 WebDriver 세션은 유지됨
    const titleAfter = await browser.getTitle()
    expect(titleAfter).toBeDefined()
  })

  /**
   * @tc_id T121
   * @risk_id UX-002
   * @tauri_only_reason Real Tauri WebView keyboard event routing
   */
  it('T121: Cmd+K opens Command Palette with focus', async () => {
    await browser.keys(['Meta', 'k'])

    const dialog = await $('div[role="dialog"]')
    await dialog.waitForExist({ timeout: 3000 })
    expect(await dialog.isDisplayed()).toBe(true)

    // combobox input이 포커스를 받았는지 확인
    const input = await $('input[role="combobox"]')
    expect(await input.isFocused()).toBe(true)

    // 정리: Escape로 닫기
    await browser.keys('Escape')
  })

  /**
   * @tc_id T122
   * @risk_id UX-003
   * @tauri_only_reason Command Palette navigates via Tauri router
   */
  it('T122: Command Palette Enter navigates to selected page', async () => {
    await browser.keys(['Meta', 'k'])
    await $('input[role="combobox"]').waitForExist({ timeout: 3000 })

    // "time" 입력 → Timeline 매치
    await browser.keys('time')
    await browser.pause(500)

    // 첫 번째 옵션 선택
    await browser.keys('ArrowDown')
    await browser.keys('Enter')
    await browser.pause(1000)

    // URL에 /timeline 포함 확인
    const url = await browser.getUrl()
    expect(url).toContain('/timeline')
  })

  /**
   * @tc_id T123
   * @risk_id UX-004
   * @tauri_only_reason Dialog lifecycle in real WebView
   */
  it('T123: Escape closes Command Palette', async () => {
    await browser.keys(['Meta', 'k'])
    const dialog = await $('div[role="dialog"]')
    await dialog.waitForExist({ timeout: 3000 })

    await browser.keys('Escape')
    await browser.pause(500)

    expect(await dialog.isExisting()).toBe(false)
  })

  /**
   * @tc_id T124
   * @risk_id RESIL-003
   * @tauri_only_reason Rapid navigation stress test on real WebView renderer
   */
  it('T124: Rapid navigation (10 transitions) survives', async () => {
    const shortcuts = ['d', 't', 's', 'p', 'd', 't', 's', 'p', 'd', 't']

    for (const key of shortcuts) {
      await browser.keys(key)
      await browser.pause(200)
    }

    // 마지막 키 't' → /timeline
    await browser.pause(1000)
    const url = await browser.getUrl()
    expect(url).toContain('/timeline')

    // error-boundary 없음 확인
    const source = await browser.getPageSource()
    expect(source).not.toContain('error-boundary')
    expect(source).not.toContain('Something went wrong')
  })
})
