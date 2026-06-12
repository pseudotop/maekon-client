use serde_json::Value;
use std::path::PathBuf;

fn maekon_client_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("maekon-core crate must live under crates/")
        .to_path_buf()
}

fn read_csp(file_name: &str) -> String {
    let path = maekon_client_root().join("src-tauri").join(file_name);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", path.display());
    });
    let json: Value = serde_json::from_str(&raw).unwrap_or_else(|error| {
        panic!("failed to parse {}: {error}", path.display());
    });
    json.pointer("/app/security/csp")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{file_name} must define app.security.csp"))
        .to_string()
}

fn csp_directive_tokens(csp: &str, directive: &str) -> Vec<String> {
    csp.split(';')
        .filter_map(|segment| {
            let tokens = segment.split_whitespace().collect::<Vec<_>>();
            if tokens.first().copied() == Some(directive) {
                Some(tokens.into_iter().skip(1).map(String::from).collect())
            } else {
                None
            }
        })
        .next()
        .unwrap_or_default()
}

#[test]
fn prod_csp_disallows_dev_eval_inline_and_allows_loopback_api_ports() {
    let csp = read_csp("tauri.conf.json");

    assert!(!csp_directive_tokens(&csp, "script-src").contains(&"'unsafe-eval'".to_string()));
    assert!(!csp_directive_tokens(&csp, "style-src").contains(&"'unsafe-inline'".to_string()));
    assert!(csp_directive_tokens(&csp, "style-src-attr").contains(&"'unsafe-inline'".to_string()));

    let connect_src = csp_directive_tokens(&csp, "connect-src");
    let img_src = csp_directive_tokens(&csp, "img-src");
    for port in 10090..=10099 {
        let origin = format!("http://127.0.0.1:{port}");
        assert!(
            connect_src.contains(&origin),
            "prod CSP connect-src must allow local Axum API/SSE origin {origin}"
        );
        assert!(
            img_src.contains(&origin),
            "prod CSP img-src must allow local frame image origin {origin}"
        );
    }
}

#[test]
fn dev_csp_keeps_dashboard_ports_in_dev_overlay_only() {
    let csp = read_csp("tauri.dev.conf.json");

    assert!(csp.contains("'unsafe-eval'"));
    assert!(csp.contains("'unsafe-inline'"));
    assert!(csp.contains("127.0.0.1:10090"));
    assert!(csp.contains("127.0.0.1:10099"));
}
