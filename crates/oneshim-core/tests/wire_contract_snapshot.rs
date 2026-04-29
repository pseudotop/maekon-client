//! Wire-format contract snapshot.
//!
//! Every addition / removal / rename of a code string MUST update
//! `wire_contract_snapshot.expected.txt` alongside the source change.
//! Per spec §7.5, released code strings are wire-immutable: deletions and
//! renames require an RFC PR justifying the wire break.

use oneshim_core::error_codes;

#[test]
fn wire_codes_match_expected_snapshot() {
    let actual: Vec<&'static str> = error_codes::all_codes();
    let mut actual_sorted = actual.clone();
    actual_sorted.sort();
    actual_sorted.dedup();

    let expected_raw = include_str!("wire_contract_snapshot.expected.txt");
    let expected: Vec<&str> = expected_raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    if actual_sorted != expected {
        let diff_added: Vec<&&str> = actual_sorted
            .iter()
            .filter(|c| !expected.contains(c))
            .collect();
        let diff_removed: Vec<&&str> = expected
            .iter()
            .filter(|c| !actual_sorted.contains(c))
            .collect();

        panic!(
            "Wire-format snapshot mismatch. \
             Added codes (not in fixture): {diff_added:?}. \
             Removed codes (fixture has them but source does not): {diff_removed:?}. \
             Update tests/wire_contract_snapshot.expected.txt to reflect the change. \
             Per spec §7.5, deletions/renames require RFC PR."
        );
    }
}
