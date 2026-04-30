//! Regression test registry.
//!
//! Every bug fix MUST include a reproducer test in this file before merge.
//!
//! ## Naming Convention
//! - Function: `test_gh{issue}_{short_description}`
//! - Example: `test_gh142_deep_merge_null_array`
//!
//! ## Policy
//! 1. Test must FAIL on the buggy code and PASS on the fix
//! 2. Tests are never deleted — only quarantined with `#[ignore]` + issue link
//! 3. Each test documents the original bug report URL
//! 4. Flaky quarantine: max 30 days — fix or delete with justification
//!
//! Run: `cargo test -p oneshim-app --test regressions`

/// Placeholder — verifies the regression harness is discovered by cargo test.
#[test]
fn regression_harness_is_wired() {
    let regression_file =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions.rs");
    assert!(
        regression_file.is_file(),
        "regression harness file should exist at {}",
        regression_file.display()
    );
}

// Future regression tests go below. Example:
//
// /// GH-142: deep_merge crashed on null array values
// /// https://github.com/pseudotop/oneshim-client/issues/142
// #[test]
// fn test_gh142_deep_merge_null_array() {
//     let mut base = serde_json::json!({"items": [1,2,3]});
//     commands::deep_merge(&mut base, serde_json::json!({"items": null}));
//     assert_eq!(base["items"], serde_json::Value::Null);
// }
