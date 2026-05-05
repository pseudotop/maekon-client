import { readFileSync } from 'node:fs'
import type { StorybookConfig } from '@storybook/react-vite'
import { finalizeStorybookViteConfig } from './vite-shared.ts'

const pkg = JSON.parse(readFileSync('./package.json', 'utf-8'))

const config: StorybookConfig = {
  stories: ['../src/**/*.stories.@(ts|tsx)'],
  addons: ['@storybook/addon-a11y', '@storybook/addon-themes', '@storybook/addon-docs'],
  framework: {
    name: '@storybook/react-vite',
    options: {},
  },
  async viteFinal(config) {
    return finalizeStorybookViteConfig(config, pkg.version)
  },
}

export default config
