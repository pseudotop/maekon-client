/// <reference types="vitest" />
import { fileURLToPath } from 'node:url'
import { defineConfig, mergeConfig } from 'vitest/config'
import viteConfig from './vite.config'

export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      environment: 'jsdom',
      globals: true,
      setupFiles: ['./src/__tests__/setup.ts'],
      include: ['src/**/*.test.{ts,tsx}'],
      resolve: {
        alias: {
          '@src': fileURLToPath(new URL('./src', import.meta.url)),
        },
      },
      coverage: {
        provider: 'v8',
        include: ['src/components/shell/**', 'src/hooks/**'],
      },
    },
  }),
)
