import { readFileSync } from 'node:fs'
import { basename, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { colors } from '../../styles/tokens'
import { darkThemeGlobals, lightThemeGlobals, reviewStoryParameters } from '../storybook-helpers'

type StoryModule = {
  LightReview?: { globals?: unknown; parameters?: unknown }
  DarkReview?: { globals?: unknown; parameters?: unknown }
}

const routePageStories = Object.entries(import.meta.glob('../../pages/*.stories.tsx', { eager: true })).map(
  ([path, module]) => [basename(path, '.stories.tsx'), module as StoryModule] as const,
)

const templateStories = Object.entries(import.meta.glob('../templates/*.stories.tsx', { eager: true })).map(
  ([path, module]) => [basename(path, '.stories.tsx'), module as StoryModule] as const,
)

describe('storybook review coverage', () => {
  it('keeps light and dark review variants for route-level pages', () => {
    for (const [name, storyModule] of routePageStories) {
      expect(storyModule.LightReview, `${name} should export LightReview`).toBeDefined()
      expect(storyModule.DarkReview, `${name} should export DarkReview`).toBeDefined()
      expect(storyModule.LightReview.globals).toEqual(lightThemeGlobals)
      expect(storyModule.DarkReview.globals).toEqual(darkThemeGlobals)
      expect(storyModule.LightReview.parameters).toEqual(reviewStoryParameters)
      expect(storyModule.DarkReview.parameters).toEqual(reviewStoryParameters)
    }
  })

  it('keeps light and dark review variants for template stories', () => {
    for (const [name, storyModule] of templateStories) {
      expect(storyModule.LightReview, `${name} should export LightReview`).toBeDefined()
      expect(storyModule.DarkReview, `${name} should export DarkReview`).toBeDefined()
      expect(storyModule.LightReview.globals).toEqual(lightThemeGlobals)
      expect(storyModule.DarkReview.globals).toEqual(darkThemeGlobals)
    }
  })

  it('keeps page title tokens and preview defaults safe for light-theme review', () => {
    expect(colors.text.pageTitle).toBe('text-content')
    expect(colors.text.pageSubtitle).toBe('text-content-secondary')

    const currentFile = fileURLToPath(import.meta.url)
    const previewPath = resolve(dirname(currentFile), '../../../.storybook/preview.ts')
    const previewSource = readFileSync(previewPath, 'utf8')
    expect(previewSource).toContain("defaultTheme: 'light'")
  })

  it('keeps permission review surfaces wired in onboarding', () => {
    // NOTE: The Settings.stories.tsx file was removed when the Settings page
    // was split into a Layout/Section structure. Permission review coverage
    // for the settings surface is expected to live in the new per-tab stories
    // once they are authored; for now we only guard the onboarding copy here.
    const currentFile = fileURLToPath(import.meta.url)
    const onboardingSourcePath = resolve(dirname(currentFile), '../../pages/Onboarding.tsx')
    const onboardingSource = readFileSync(onboardingSourcePath, 'utf8')
    expect(onboardingSource).toContain('step2DescWindows')
    expect(onboardingSource).toContain('step2DescLinux')
  })
})
