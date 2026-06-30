use super::*;

// -------------------------------------------------------------------
// Task 7: Staged Rollout Tests (FNV-1a bucketing + rollout parsing)
// -------------------------------------------------------------------

#[test]
fn fnv1a_hash_deterministic() {
    let h1 = fnv1a_hash(b"test-device-v1.0.0");
    let h2 = fnv1a_hash(b"test-device-v1.0.0");
    assert_eq!(h1, h2);
}

#[test]
fn fnv1a_hash_different_inputs() {
    let h1 = fnv1a_hash(b"device-a-v1.0.0");
    let h2 = fnv1a_hash(b"device-b-v1.0.0");
    assert_ne!(h1, h2);
}

#[test]
fn rollout_100_always_eligible() {
    assert!(is_eligible_for_rollout("any-device", "v1.0.0", 100));
}

#[test]
fn rollout_0_never_eligible() {
    assert!(!is_eligible_for_rollout("any-device", "v1.0.0", 0));
}

#[test]
fn rollout_deterministic() {
    let r1 = is_eligible_for_rollout("device-123", "v2.0.0", 50);
    let r2 = is_eligible_for_rollout("device-123", "v2.0.0", 50);
    assert_eq!(r1, r2);
}

#[test]
fn parse_rollout_present() {
    let body = Some("<!-- rollout:25 -->\n## Changes".to_string());
    assert_eq!(parse_rollout_percent(&body), 25);
}

#[test]
fn parse_rollout_absent() {
    let body = Some("## Changes\n- Fix bugs".to_string());
    assert_eq!(parse_rollout_percent(&body), 0);
}

#[test]
fn parse_rollout_none() {
    assert_eq!(parse_rollout_percent(&None), 0);
}

#[test]
fn parse_rollout_caps_at_100() {
    let body = Some("<!-- rollout:150 -->".to_string());
    assert_eq!(parse_rollout_percent(&body), 100);
}

#[test]
fn parse_rollout_malformed_fails_closed() {
    let body = Some("<!-- rollout:abc -->".to_string());
    assert_eq!(parse_rollout_percent(&body), 0);
}

// ── D10 defensive None handling + rollout-gate end-to-end ─────────

/// When a release body contains `<!-- rollout:0 -->`, every installation
/// is excluded from the rollout bucket. `check_for_updates` must return
/// `UpToDate` (no update offered) even though the semver comparison
/// reports an available newer version.
#[tokio::test]
async fn update_check_respects_rollout_exclusion() {
    let mut server = mockito::Server::new_async().await;
    let newer_version = "99.0.0";

    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
            "tag_name": "v{}",
            "name": "Rollout Excluded",
            "body": "new features\n\n<!-- rollout:0 -->\n",
            "prerelease": false,
            "assets": [],
            "html_url": "https://github.com/test/releases/v99.0.0",
            "published_at": "2024-01-01T00:00:00Z"
        }}"#,
            newer_version
        ))
        .create_async()
        .await;

    let config = test_config(); // installation_id = Some("test-install-...")
    let updater = Updater::new(config);

    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;

    match result {
        Ok(UpdateCheckResult::UpToDate { .. }) => {
            // Expected: rollout:0 excludes every device.
        }
        other => unreachable!("Expected UpToDate on rollout:0, got {:?}", other),
    }
}

/// When `installation_id` is `None` at check time (regression against the
/// invariant that `app_runtime_launch.rs:66-74` writes a UUID before any
/// update check spawns), the updater must treat the device as
/// rollout-EXCLUDED rather than admitting it as always-eligible.
#[tokio::test]
async fn update_check_without_installation_id_is_excluded() {
    let mut server = mockito::Server::new_async().await;
    let newer_version = "99.0.0";

    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
            "tag_name": "v{}",
            "name": "Rollout 100%",
            "body": "new features",
            "prerelease": false,
            "assets": [],
            "html_url": "https://github.com/test/releases/v99.0.0",
            "published_at": "2024-01-01T00:00:00Z"
        }}"#,
            newer_version
        ))
        .create_async()
        .await;

    let mut config = test_config();
    config.installation_id = None; // regression scenario

    let updater = Updater::new(config);

    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;

    match result {
        Ok(UpdateCheckResult::UpToDate { .. }) => {
            // Expected: None → defensive-exclude even at rollout:100.
        }
        other => unreachable!("Expected UpToDate on None installation_id, got {:?}", other),
    }
}
