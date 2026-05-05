import { resolve } from 'node:path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { readFileSync } from 'node:fs'
import { DEFAULT_WEB_PORT } from './src/constants'

const pkg = JSON.parse(readFileSync('./package.json', 'utf-8'))

// Read workspace version from Cargo.toml (single source of truth), fallback to package.json
function resolveVersion(): string {
  try {
    const cargoToml = readFileSync('../../../Cargo.toml', 'utf-8')
    const match = cargoToml.match(/^\s*version\s*=\s*"([^"]+)"/m)
    if (match) return match[1]
  } catch {
    // fallback to package.json
  }
  return pkg.version
}

export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(`v${resolveVersion()}`),
  },
  plugins: [react()],
  server: {
    proxy: {
      '/api': {
        target: `http://localhost:${DEFAULT_WEB_PORT}`,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
        overlay: resolve(__dirname, 'overlay.html'),
        'tracking-panel': resolve(__dirname, 'tracking-panel.html'),
      },
      output: {
        manualChunks: {
          vendor: ['react', 'react-dom'],
          charts: ['recharts'],
          query: ['@tanstack/react-query'],
          ui: ['lucide-react'],
          router: ['react-router-dom'],
          i18n: ['i18next', 'react-i18next', 'i18next-browser-languagedetector'],
          markdown: ['react-markdown', 'remark-gfm', 'react-syntax-highlighter'],
        },
      },
    },
  },
})
