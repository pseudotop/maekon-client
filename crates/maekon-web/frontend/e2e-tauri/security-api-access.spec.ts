// e2e-tauri/security-api-access.spec.ts
import http from 'node:http'
import { fetchApiJson, invokeIpc } from './helpers.js'

type SecuritySettingsSnapshot = {
  server?: { base_url?: string }
  ai_provider?: {
    ocr_api?: { api_key?: string }
    llm_api?: { api_key?: string }
  }
  tls?: { ca_cert_path?: string }
}

/**
 * Calls the REST API from outside using a Node.js HTTP client.
 * WebdriverIO tests run under Node.js, so direct HTTP requests are possible.
 */
function httpRequest(
  url: string,
  method: string,
  headers?: Record<string, string>,
): Promise<{ status: number; headers: http.IncomingHttpHeaders; body: string }> {
  return new Promise((resolve, reject) => {
    const parsed = new URL(url)
    const req = http.request(
      {
        hostname: parsed.hostname,
        port: parsed.port,
        path: parsed.pathname,
        method,
        headers: headers || {},
      },
      (res) => {
        let body = ''
        res.on('data', (chunk) => (body += chunk))
        res.on('end', () => resolve({ status: res.statusCode || 0, headers: res.headers, body }))
      },
    )
    req.on('error', reject)
    req.end()
  })
}

describe('S1: API Access Control', () => {
  let webPort: number

  before(async () => {
    webPort = await invokeIpc<number>('get_web_port')
  })

  /**
   * @tc_id T201
   * @risk_id SEC-011
   * @tauri_only_reason Tests CORS from external origin against real Axum server
   */
  it('T201: CORS rejects cross-origin request from evil.com', async () => {
    const res = await httpRequest(`http://127.0.0.1:${webPort}/api/metrics`, 'OPTIONS', {
      Origin: 'https://evil.com',
      'Access-Control-Request-Method': 'GET',
    })

    // The CORS preflight must not allow evil.com
    const acao = res.headers['access-control-allow-origin']
    expect(acao).not.toBe('*')
    if (acao) {
      expect(acao).not.toBe('https://evil.com')
    }
  })

  /**
   * @tc_id T202
   * @risk_id SEC-012
   * @tauri_only_reason REST /api/settings returns the real desktop config snapshot from disk
   */
  it('T202: get_settings does not expose server credentials', async () => {
    const config = await fetchApiJson<SecuritySettingsSnapshot>('/settings')

    // server.base_url must be masked
    if (config.server) {
      expect(config.server.base_url).toBe('[REDACTED]')
    }

    // The ai_provider API keys must be masked
    if (config.ai_provider) {
      if (config.ai_provider.ocr_api) {
        expect(config.ai_provider.ocr_api.api_key).toBe('[REDACTED]')
      }
      if (config.ai_provider.llm_api) {
        expect(config.ai_provider.llm_api.api_key).toBe('[REDACTED]')
      }
    }

    // The TLS certificate path must be masked
    if (config.tls) {
      expect(config.tls.ca_cert_path || '[REDACTED]').toBe('[REDACTED]')
    }
  })

  /**
   * @tc_id T204
   * @risk_id SEC-014
   * @tauri_only_reason Tests CORS wildcard absence on real Axum server
   */
  it('T204: CORS does not return Access-Control-Allow-Origin: *', async () => {
    const res = await httpRequest(`http://127.0.0.1:${webPort}/api/metrics`, 'GET', {
      Origin: 'https://attacker.example.com',
    })

    const acao = res.headers['access-control-allow-origin']
    expect(acao).not.toBe('*')
  })
})
