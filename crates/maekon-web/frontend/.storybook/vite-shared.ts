import type { ManualChunkMeta, OutputOptions } from 'rollup'
import { type InlineConfig, mergeConfig } from 'vite'

const STORYBOOK_MOCKING_PLUGIN_NAMES = [
  'vite:storybook-inject-mocker-runtime',
  'storybook:mock-loader',
  'storybook:mock-loader-preview',
]

function isNamedPlugin(plugin: unknown): plugin is { name: string } {
  return (
    typeof plugin === 'object' &&
    plugin !== null &&
    'name' in plugin &&
    typeof (plugin as { name?: unknown }).name === 'string'
  )
}

function stripStorybookMockingPlugins(plugins: InlineConfig['plugins']): InlineConfig['plugins'] {
  if (!plugins) {
    return plugins
  }

  const strip = (plugin: unknown): unknown => {
    if (Array.isArray(plugin)) {
      const nested = plugin.map(strip).filter(Boolean)
      return nested.length > 0 ? nested : null
    }

    if (isNamedPlugin(plugin) && STORYBOOK_MOCKING_PLUGIN_NAMES.includes(plugin.name)) {
      return null
    }

    return plugin
  }

  if (Array.isArray(plugins)) {
    return plugins.map(strip).filter(Boolean) as InlineConfig['plugins']
  }

  return strip(plugins) as InlineConfig['plugins']
}

function packageNameFromModuleId(id: string): string | null {
  const nodeModulesMarker = '/node_modules/'
  const nodeModulesIndex = id.lastIndexOf(nodeModulesMarker)
  if (nodeModulesIndex === -1) {
    return null
  }

  const modulePath = id.slice(nodeModulesIndex + nodeModulesMarker.length)
  const segments = modulePath.split('/')
  if (segments.length === 0) {
    return null
  }

  if (segments[0].startsWith('@') && segments.length >= 2) {
    return `${segments[0]}/${segments[1]}`
  }

  return segments[0] || null
}

function resolveStorybookChunk(id: string): string | undefined {
  const packageName = packageNameFromModuleId(id)
  if (!packageName) {
    return undefined
  }

  if (packageName === 'react' || packageName === 'react-dom' || packageName === 'scheduler') {
    return 'sb-react-vendor'
  }

  if (packageName === 'react-router' || packageName === 'react-router-dom') {
    return 'sb-app-router'
  }

  if (packageName === '@tanstack/react-query') {
    return 'sb-app-query'
  }

  if (packageName === 'recharts' || packageName.startsWith('d3-')) {
    return 'sb-app-charts'
  }

  if (
    packageName === 'react-markdown' ||
    packageName === 'react-syntax-highlighter' ||
    packageName === 'remark-gfm' ||
    packageName.startsWith('remark-') ||
    packageName.startsWith('rehype-') ||
    packageName.startsWith('micromark') ||
    packageName.startsWith('mdast-') ||
    packageName.startsWith('hast-')
  ) {
    return 'sb-app-markdown'
  }

  if (packageName === 'axe-core' || packageName === '@storybook/addon-a11y') {
    return 'sb-addon-a11y'
  }

  if (
    packageName === '@storybook/addon-docs' ||
    packageName === 'markdown-to-jsx' ||
    packageName.startsWith('@mdx-js/')
  ) {
    return 'sb-addon-docs'
  }

  return undefined
}

type ManualChunks = Record<string, string[]> | ((id: string, meta: ManualChunkMeta) => string | undefined) | undefined

function createManualChunks(existingManualChunks: ManualChunks) {
  return (id: string, meta: ManualChunkMeta): string | undefined => {
    const storybookChunk = resolveStorybookChunk(id)
    if (storybookChunk) {
      return storybookChunk
    }

    if (typeof existingManualChunks === 'function') {
      return existingManualChunks(id, meta)
    }

    return undefined
  }
}

function resolveOutputOptions(config: InlineConfig): OutputOptions {
  const currentOutput = config.build?.rollupOptions?.output
  if (Array.isArray(currentOutput)) {
    return {}
  }

  const output = currentOutput ?? {}
  return {
    ...output,
    manualChunks: createManualChunks(output.manualChunks),
  }
}

export function applyStorybookViteConfig(
  config: InlineConfig,
  pkgVersion: string,
  chunkSizeWarningLimit = 700,
): InlineConfig {
  return mergeConfig(config, {
    define: {
      __APP_VERSION__: JSON.stringify(`v${pkgVersion}`),
    },
    build: {
      chunkSizeWarningLimit,
      rollupOptions: {
        output: resolveOutputOptions(config),
      },
    },
  })
}

export async function finalizeStorybookViteConfig(
  config: InlineConfig,
  pkgVersion: string,
  chunkSizeWarningLimit = 700,
): Promise<InlineConfig> {
  const merged = applyStorybookViteConfig(config, pkgVersion, chunkSizeWarningLimit)
  return {
    ...merged,
    plugins: stripStorybookMockingPlugins(merged.plugins),
  }
}
