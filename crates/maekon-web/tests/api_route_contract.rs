//! REST API route contract tests — CRT-PRV-API-001..020.
//!
//! Each `#[test]` corresponds to one CRT-PRV-API-NNN route contract. The test
//! asserts that the named prefix appears in
//! `crates/maekon-web/src/routes.rs` so the contract surface stays declared
//! even when the implementation churns underneath.
//!
//! These are SMOKE / SURFACE tests, not behavior tests. Pass = the route
//! exists in the router builder; behavior tests live elsewhere (handlers/
//! their own tests, e2e-live).
//!
//! Run via:
//!   cargo test -p maekon-web --test api_route_contract

use std::fs;
use std::path::PathBuf;

fn load_routes_source() -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let routes_path = manifest_dir.join("src/routes.rs");
    fs::read_to_string(&routes_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", routes_path.display(), e))
}

fn assert_route_prefix(prefix: &str) {
    let src = load_routes_source();
    // Routes use both single-line (`.route("/x", ...)`) and multi-line
    // (`.route(\n  "/x",\n  ...,\n)`) builder forms. Match the bare prefix
    // appearing as a quoted string near a `.route(` call.
    let needle = format!("\"{prefix}");
    assert!(
        src.contains(&needle),
        "Expected api_routes() to declare a route with `{prefix}` but no \
         `\"{prefix}...\"` found in routes.rs"
    );
}

#[test]
fn crt_prv_api_001_metrics() {
    // /metrics — system metrics time-series query
    assert_route_prefix("/metrics");
}

#[test]
fn crt_prv_api_002_processes() {
    // /processes — process snapshot query
    assert_route_prefix("/processes");
}

#[test]
fn crt_prv_api_003_idle() {
    // /idle — idle period query
    assert_route_prefix("/idle");
}

#[test]
fn crt_prv_api_004_sessions() {
    // /sessions, /sessions/{id} — session list + detail
    assert_route_prefix("/sessions");
}

#[test]
fn crt_prv_api_005_frames() {
    // /frames, /frames/{id}/image — frame query + binary stream
    assert_route_prefix("/frames");
}

#[test]
fn crt_prv_api_006_events() {
    // /events — event log query
    assert_route_prefix("/events");
}

#[test]
fn crt_prv_api_007_stats() {
    // /stats/* — summary, apps, heatmap
    assert_route_prefix("/stats");
}

#[test]
fn crt_prv_api_008_reports() {
    // /reports — report generation
    assert_route_prefix("/reports");
}

#[test]
fn crt_prv_api_009_settings() {
    // /settings — get/update settings
    assert_route_prefix("/settings");
}

#[test]
fn crt_prv_api_010_ai_provider_surfaces() {
    // /ai/provider-surfaces — AI provider catalog
    assert_route_prefix("/ai/");
}

#[test]
fn crt_prv_api_011_annotations() {
    // /frames/{id}/annotations — annotation CRUD (under frames prefix)
    let src = load_routes_source();
    assert!(
        src.contains("/annotations"),
        "Expected annotations sub-route to appear in routes.rs"
    );
}

#[test]
fn crt_prv_api_012_automation() {
    // /automation/* — preset + audit
    assert_route_prefix("/automation");
}

#[test]
fn crt_prv_api_013_tags() {
    // /tags — tag CRUD
    assert_route_prefix("/tags");
}

#[test]
fn crt_prv_api_014_search() {
    // /search — fulltext search
    assert_route_prefix("/search");
}

#[test]
fn crt_prv_api_015_export() {
    // /export — data export
    assert_route_prefix("/export");
}

#[test]
fn crt_prv_api_016_backup() {
    // /backup — backup/restore
    assert_route_prefix("/backup");
}

#[test]
fn crt_prv_api_017_focus() {
    // /focus/* — focus metrics + work sessions + interruptions
    assert_route_prefix("/focus");
}

#[test]
fn crt_prv_api_018_coaching() {
    // /coaching/* — coaching templates + messages
    assert_route_prefix("/coaching");
}

#[test]
fn crt_prv_api_019_bug_report() {
    // /bug_report or /support — bug report submission
    let src = load_routes_source();
    assert!(
        src.contains("/bug") || src.contains("/support"),
        "Expected bug-report endpoint (/bug_report or /support) in routes.rs"
    );
}

#[test]
fn crt_prv_api_020_update_stream() {
    // /update/stream — SSE update channel
    let src = load_routes_source();
    assert!(
        src.contains("/update"),
        "Expected /update endpoint in routes.rs"
    );
}
