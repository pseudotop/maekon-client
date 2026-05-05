import { readFileSync } from 'node:fs'
import type { StorybookConfig } from '@storybook/react-vite'
import { finalizeStorybookViteConfig } from '../.storybook/vite-shared.ts'

const pkg = JSON.parse(readFileSync('./package.json', 'utf-8'))

const config: StorybookConfig = {
  stories: ['../src/**/*.stories.@(ts|tsx)'],
  addons: ['@storybook/addon-themes'],
  framework: {
    name: '@storybook/react-vite',
    options: {},
  },
  async viteFinal(config) {
    return finalizeStorybookViteConfig(config, pkg.version, 1400)
  },
}

export default config
