use axum::http::{header, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
#[cfg(maekon_web_embedded_dist)]
use rust_embed::Embed;

#[cfg(maekon_web_embedded_dist)]
#[derive(Embed)]
#[folder = "frontend/dist"]
#[include = "*.html"]
#[include = "*.js"]
#[include = "*.css"]
#[include = "*.svg"]
#[include = "*.png"]
#[include = "*.ico"]
#[include = "*.json"]
#[include = "*.woff"]
#[include = "*.woff2"]
#[include = "assets/**/*"]
struct Assets;

#[cfg(not(maekon_web_embedded_dist))]
struct Assets;

#[cfg(not(maekon_web_embedded_dist))]
impl Assets {
    fn get(_path: &str) -> Option<rust_embed::EmbeddedFile> {
        None
    }
}

#[allow(clippy::unused_async)] // axum handler requires async return
pub async fn serve_static(uri: Uri) -> Response {
    serve_static_impl(uri)
}

/// #6281: security headers for the loopback-HTTP-served dashboard. The Tauri
/// WebView has its own CSP, but the dashboard is ALSO reachable over plain HTTP
/// on localhost (a browser), where no CSP/nosniff applies otherwise.
/// `X-Content-Type-Options: nosniff` blocks MIME sniffing; the CSP is permissive
/// enough for a self-contained Vite SPA (self scripts, inline styles for
/// Tailwind, self/data images, self API connections) while denying objects,
/// framing, and base-uri hijack.
const STATIC_CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; \
object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

fn serve_static_impl(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();

            let cache_control = if path.ends_with(".html") {
                "no-cache"
            } else if path.contains("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "public, max-age=3600"
            };

            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime.as_ref()),
                    (header::CACHE_CONTROL, cache_control),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                    (header::CONTENT_SECURITY_POLICY, STATIC_CSP),
                    (header::REFERRER_POLICY, "no-referrer"),
                ],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => {
            if let Some(index) = Assets::get("index.html") {
                // #6281: SPA client-route fallback — return the index shell with
                // the SAME no-cache + security headers as the matched .html arm,
                // not a bare Html(...) with no Cache-Control / security headers.
                (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                        (header::CACHE_CONTROL, "no-cache"),
                        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                        (header::CONTENT_SECURITY_POLICY, STATIC_CSP),
                        (header::REFERRER_POLICY, "no-referrer"),
                    ],
                    index.data.into_owned(),
                )
                    .into_response()
            } else {
                (StatusCode::OK, Html(DEV_PLACEHOLDER.to_string())).into_response()
            }
        }
    }
}

const DEV_PLACEHOLDER: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Maekon Dashboard</title>
    <style>
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            color: #e0e0e0;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .container {
            text-align: center;
            padding: 40px;
            max-width: 600px;
        }
        h1 {
            font-size: 2.5rem;
            margin-bottom: 1rem;
            background: linear-gradient(90deg, #00d9ff, #00ff88);
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
        }
        .subtitle {
            color: #888;
            margin-bottom: 2rem;
        }
        .status {
            background: rgba(255,255,255,0.05);
            border-radius: 12px;
            padding: 24px;
            margin-bottom: 2rem;
        }
        .status h2 {
            color: #00d9ff;
            margin-bottom: 1rem;
        }
        .api-list {
            text-align: left;
            list-style: none;
        }
        .api-list li {
            padding: 8px 0;
            border-bottom: 1px solid rgba(255,255,255,0.1);
        }
        .api-list code {
            background: rgba(0,217,255,0.1);
            padding: 2px 8px;
            border-radius: 4px;
            font-family: 'SF Mono', monospace;
        }
        .build-hint {
            background: #2d2d44;
            padding: 16px;
            border-radius: 8px;
            font-family: 'SF Mono', monospace;
            font-size: 0.9rem;
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>Maekon</h1>
        <p class="subtitle">Local Web Dashboard</p>

        <div class="status">
            <h2>✅ API server running</h2>
            <ul class="api-list">
                <li><code>GET /api/stats/summary</code> - Today's summary</li>
                <li><code>GET /api/metrics</code> - System metrics</li>
                <li><code>GET /api/processes</code> - Process snapshot</li>
                <li><code>GET /api/frames</code> - Screenshot list</li>
                <li><code>GET /api/events</code> - Event log</li>
                <li><code>GET /api/idle</code> - idle period</li>
                <li><code>GET /api/sessions</code> - session list</li>
            </ul>
        </div>

        <p style="margin-bottom: 1rem; color: #888;">Frontend build:</p>
        <div class="build-hint">
            cd crates/maekon-web/frontend<br>
            pnpm install && pnpm build
        </div>
    </div>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_placeholder_is_valid_html() {
        assert!(DEV_PLACEHOLDER.contains("<!DOCTYPE html>"));
        assert!(DEV_PLACEHOLDER.contains("Maekon"));
    }
}
