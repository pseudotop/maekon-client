import { createRoot } from 'react-dom/client'
import { installFrontendLogBridge } from '../logging/frontendLogger'
import '../i18n'
import { App } from './App'
import '../index.css'

installFrontendLogBridge('tracking-panel')

// biome-ignore lint/style/noNonNullAssertion: root element guaranteed by panel.html
createRoot(document.getElementById('panel-root')!).render(<App />)
