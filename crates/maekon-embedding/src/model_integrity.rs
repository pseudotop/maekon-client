//! Supply-chain integrity controls for the locally-downloaded embedding model
//! (#7082 / MEMB-3).
//!
//! `fastembed` lazily downloads ONNX weights from a built-in Hugging Face repo
//! on first use and hands them straight to ONNX Runtime. Without an integrity
//! control, an upstream repo change or account compromise would cause a
//! different ONNX graph to be loaded and executed by the provider. This module
//! pins the exact bytes we accept:
//!
//! 1. `PINNED_REVISIONS` records the Hugging Face *commit* (not a moving tag
//!    such as `main`) that each allowlisted digest was captured at. It is the
//!    provenance of every digest. fastembed 5.x's `InitOptions` exposes no
//!    revision parameter and `pull_from_hf` resolves the repo's default
//!    (branch-tracking) ref, so verification reads hf-hub's `refs/main` cache
//!    entry and rejects the model if the loaded snapshot commit differs from
//!    the pinned revision.
//! 2. `WEIGHT_DIGESTS` is a SHA-256 allowlist of the ONNX weights file, keyed by
//!    model id. **Content pinning via SHA-256 is strictly stronger than revision
//!    pinning**: a matching digest guarantees the exact bytes of the pinned
//!    commit regardless of where the branch currently points.
//!
//! Enforcement is fail-closed for any model whose digest is registered: a
//! mismatch (or a missing-but-expected file) is rejected before the provider is
//! returned for use. A model with no digest captured yet is allowed through
//! with a warning (dormant) so an unrecognised/future model id still works.
//!
//! All eight models that `resolve_model` can select are ENFORCED: their digests
//! were back-filled from the upstream Hugging Face repos at the pinned commits
//! recorded in `PINNED_REVISIONS` (see `WEIGHT_DIGESTS` for per-model
//! provenance, source URLs and the retrieval date — #7102).
//!
//! ## ort 2.0 tracking
//! This crate resolves `ort` / `ort-sys` `2.0.0-rc.12`, a pre-release that
//! downloads the ONNX Runtime native binary at build time. Move to a stable
//! `ort` `2.0` release once published and prefer a pinned / reproducible
//! native-binary acquisition. Tracked by #7082.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::EmbeddingError;

/// SHA-256 allowlist of accepted ONNX weights, keyed by the stable model id
/// returned by `resolve_model` (e.g. `"all-MiniLM-L6-v2-Q"`).
///
/// An entry here is ENFORCED (fail-closed); a model with no entry is accepted
/// with a warning (dormant). Every key maps 1:1 to a `resolve_model` model id.
///
/// Each digest is the lowercase-hex SHA-256 of the exact ONNX weights file that
/// `fastembed` downloads for the model (`model_code` repo + `model_file`),
/// captured at the commit recorded in `PINNED_REVISIONS`. A git LFS `oid sha256`
/// is, by the LFS spec, the SHA-256 of the file content — i.e. exactly what
/// `check_digest` recomputes from the cached bytes. Values were retrieved from
/// the HF git LFS pointer (`https://huggingface.co/<repo>/raw/<commit>/<file>`)
/// and independently cross-checked against the HF API tree (`.lfs.oid`) on
/// 2026-06-28 (#7102).
///
/// | model id | HF repo (`model_code`) | weights (`model_file`) |
/// |----------|------------------------|------------------------|
/// | all-MiniLM-L6-v2-Q  | Xenova/all-MiniLM-L6-v2          | onnx/model_quantized.onnx |
/// | all-MiniLM-L12-v2-Q | Xenova/all-MiniLM-L12-v2         | onnx/model_quantized.onnx |
/// | bge-small-en-v1.5-Q | Qdrant/bge-small-en-v1.5-onnx-Q  | model_optimized.onnx      |
/// | bge-base-en-v1.5-Q  | Qdrant/bge-base-en-v1.5-onnx-Q   | model_optimized.onnx      |
/// | all-MiniLM-L6-v2    | Qdrant/all-MiniLM-L6-v2-onnx     | model.onnx                |
/// | all-MiniLM-L12-v2   | Xenova/all-MiniLM-L12-v2         | onnx/model.onnx           |
/// | bge-small-en-v1.5   | Xenova/bge-small-en-v1.5         | onnx/model.onnx           |
/// | bge-base-en-v1.5    | Xenova/bge-base-en-v1.5          | onnx/model.onnx           |
const WEIGHT_DIGESTS: &[(&str, &str)] = &[
    // Default model (quantized MiniLM-L6).
    (
        "all-MiniLM-L6-v2-Q",
        "afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1",
    ),
    (
        "all-MiniLM-L12-v2-Q",
        "f51725bc66b2bf5335cacb5c005763b57bcd741172372795819741cd945a9dd9",
    ),
    (
        "bge-small-en-v1.5-Q",
        "51f1bd0addd6e859e42c2c8021a5e5461385bb676a649f4b269aa445449f2431",
    ),
    (
        "bge-base-en-v1.5-Q",
        "4e556722bc4f65716c544c8a931f1e90fb3f866e5741fd93a96f051d673339c7",
    ),
    (
        "all-MiniLM-L6-v2",
        "bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5",
    ),
    (
        "all-MiniLM-L12-v2",
        "ac10e6b99832408b7a505c07226d48b1c7d4d7fbd12bf3421095ad0ce31fff51",
    ),
    (
        "bge-small-en-v1.5",
        "828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35",
    ),
    (
        "bge-base-en-v1.5",
        "9bc579acdba21c253c62a9bf866891355a63ffa3442b52c8a37d75b2ccb91848",
    ),
];

/// Pinned Hugging Face commit per model id — the provenance of the matching
/// `WEIGHT_DIGESTS` entry. These are the default-branch (`main`) heads resolved
/// via the HF API (`/api/models/<repo>` → `.sha`) on 2026-06-28; each digest
/// above was captured at the commit recorded here (#7102). Note the L12 full and
/// quantized variants share one repo/commit (different `model_file`).
const PINNED_REVISIONS: &[(&str, &str)] = &[
    (
        "all-MiniLM-L6-v2-Q",
        "751bff37182d3f1213fa05d7196b954e230abad9",
    ),
    (
        "all-MiniLM-L12-v2-Q",
        "beeb2e4b69e95f188a15cc2e90d09fd035dac229",
    ),
    (
        "bge-small-en-v1.5-Q",
        "52398278842ec682c6f32300af41344b1c0b0bb2",
    ),
    (
        "bge-base-en-v1.5-Q",
        "738cad1c108e2f23649db9e44b2eab988626493b",
    ),
    (
        "all-MiniLM-L6-v2",
        "5f1b8cd78bc4fb444dd171e59b18f3a3af89a079",
    ),
    (
        "all-MiniLM-L12-v2",
        "beeb2e4b69e95f188a15cc2e90d09fd035dac229",
    ),
    (
        "bge-small-en-v1.5",
        "ea104dacec62c0de699686887e3f920caeb4f3e3",
    ),
    (
        "bge-base-en-v1.5",
        "4d6cd88e18e51a5e020c2c305726d76ada9c03cf",
    ),
];

/// Outcome of a (non-failing) integrity verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityOutcome {
    /// A digest was pinned and the downloaded weights matched it.
    Verified,
    /// No digest is pinned for this model yet — verification skipped (dormant).
    Skipped,
}

/// Expected SHA-256 (lowercase hex) of the ONNX weights for `model_id`, if a
/// digest has been pinned.
#[must_use]
pub fn expected_digest(model_id: &str) -> Option<&'static str> {
    lookup(WEIGHT_DIGESTS, model_id)
}

/// Pinned Hugging Face commit for `model_id`, if recorded.
#[must_use]
pub fn pinned_revision(model_id: &str) -> Option<&'static str> {
    lookup(PINNED_REVISIONS, model_id)
}

fn lookup(table: &[(&str, &'static str)], model_id: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(id, _)| *id == model_id)
        .map(|(_, value)| *value)
}

/// Lowercase-hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for &byte in digest.iter() {
        use std::fmt::Write as _;
        // Infallible for a String sink.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Compare `bytes` against the `expected` digest for `model_id`. Fail-closed:
/// any mismatch is an error so a tampered / changed ONNX graph is never used.
///
/// # Errors
/// Returns [`EmbeddingError::Integrity`] when the computed SHA-256 differs from
/// `expected`.
pub fn check_digest(model_id: &str, expected: &str, bytes: &[u8]) -> Result<(), EmbeddingError> {
    let actual = sha256_hex(bytes);
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(EmbeddingError::Integrity(format!(
            "SHA-256 mismatch for embedding model {model_id}: expected {expected}, got {actual} \
             — refusing to load weights that differ from the pinned allowlist (#7082 MEMB-3)"
        )))
    }
}

/// App-controlled weights cache directory.
///
/// Mirrors fastembed's own precedence so we locate the same files it downloads:
/// `$HF_HOME` wins (fastembed's `pull_from_hf` honours it over `InitOptions`),
/// then `$MAEKON_EMBEDDING_CACHE_DIR`, else a stable default so repeated runs
/// reuse — and re-verify — the same weights.
#[must_use]
pub fn cache_dir() -> PathBuf {
    for var in ["HF_HOME", "MAEKON_EMBEDDING_CACHE_DIR"] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                return PathBuf::from(value);
            }
        }
    }
    PathBuf::from(".fastembed_cache")
}

/// Locate the ONNX weights file in the hf-hub cache layout
/// (`<cache>/models--<org>--<name>/snapshots/<commit>/<weight_file>`).
#[must_use]
pub fn locate_weight(
    cache_dir: &Path,
    hf_repo: &str,
    revision: &str,
    weight_file: &str,
) -> Option<PathBuf> {
    let repo_dir = cache_dir.join(format!("models--{}", hf_repo.replace('/', "--")));
    let candidate = repo_dir
        .join("snapshots")
        .join(revision.trim())
        .join(weight_file);
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn cached_main_revision(cache_dir: &Path, hf_repo: &str) -> Option<String> {
    let repo_dir = cache_dir.join(format!("models--{}", hf_repo.replace('/', "--")));
    let revision = std::fs::read_to_string(repo_dir.join("refs").join("main")).ok()?;
    let revision = revision.trim();
    (!revision.is_empty()).then(|| revision.to_string())
}

/// Verify the downloaded weights for `model_id` against the SHA-256 allowlist.
///
/// `hf_repo` / `weight_file` come from fastembed's `ModelInfo`, so this module
/// does not duplicate fastembed's model table. Fail-closed when a digest is
/// pinned: a mismatch, or an expected-but-missing file, is an error.
///
/// # Errors
/// Returns [`EmbeddingError::Integrity`] when a pinned digest does not match, or
/// when the weights file cannot be located / read while a digest is pinned.
pub fn verify_cached_weights(
    model_id: &str,
    cache_dir: &Path,
    hf_repo: &str,
    weight_file: &str,
) -> Result<IntegrityOutcome, EmbeddingError> {
    verify_with(
        expected_digest(model_id),
        model_id,
        cache_dir,
        hf_repo,
        weight_file,
    )
}

fn verify_with(
    expected: Option<&str>,
    model_id: &str,
    cache_dir: &Path,
    hf_repo: &str,
    weight_file: &str,
) -> Result<IntegrityOutcome, EmbeddingError> {
    let Some(expected) = expected else {
        tracing::warn!(
            model = model_id,
            "no SHA-256 digest pinned for this embedding model (not in the #7102 allowlist) — \
             integrity verification skipped (dormant)"
        );
        return Ok(IntegrityOutcome::Skipped);
    };
    let Some(pinned_revision) = pinned_revision(model_id) else {
        return Err(EmbeddingError::Integrity(format!(
            "pinned revision for embedding model {model_id} is missing while a digest is enforced \
             — refusing to verify ambiguous weights (#7102)"
        )));
    };
    let Some(loaded_revision) = cached_main_revision(cache_dir, hf_repo) else {
        return Err(EmbeddingError::Integrity(format!(
            "Hugging Face cache ref main for {hf_repo} is missing under cache {} — cannot verify \
             loaded weights against pinned revision {pinned_revision} (#7082 MEMB-3)",
            cache_dir.display()
        )));
    };
    if loaded_revision != pinned_revision {
        return Err(EmbeddingError::Integrity(format!(
            "loaded Hugging Face revision {loaded_revision} for embedding model {model_id} \
             ({hf_repo}) does not match pinned revision {pinned_revision} — refusing to load \
             branch-tracking weights (#7082 MEMB-3)"
        )));
    }
    let Some(path) = locate_weight(cache_dir, hf_repo, &loaded_revision, weight_file) else {
        return Err(EmbeddingError::Integrity(format!(
            "weights file {weight_file} for {model_id} not found at pinned revision \
             {pinned_revision} under cache {} — cannot verify integrity (#7082 MEMB-3)",
            cache_dir.display()
        )));
    };
    let bytes = std::fs::read(&path).map_err(|e| {
        EmbeddingError::Integrity(format!(
            "failed to read embedding weights {} for integrity check: {e}",
            path.display()
        ))
    })?;
    check_digest(model_id, expected, &bytes)?;
    Ok(IntegrityOutcome::Verified)
}

#[cfg(test)]
mod tests {
    use super::*;

    // SHA-256("hello") — a fixed, independently-verifiable vector.
    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(sha256_hex(b"hello"), HELLO_SHA256);
    }

    #[test]
    fn check_digest_accepts_match_and_rejects_mismatch() {
        // Match → Ok.
        check_digest("m", HELLO_SHA256, b"hello").expect("hello must match its own SHA-256 digest");
        // Any tampering flips the digest → fail-closed Err (the core control).
        let err = check_digest("m", HELLO_SHA256, b"tampered").unwrap_err();
        assert!(
            matches!(err, EmbeddingError::Integrity(ref msg) if msg.contains("SHA-256 mismatch")),
            "tampered weights must be rejected with an Integrity error: {err}"
        );
        // Case-insensitive hex comparison.
        check_digest("m", &HELLO_SHA256.to_uppercase(), b"hello")
            .expect("uppercase hex must compare case-insensitively");
    }

    #[test]
    fn locate_weight_finds_file_in_hf_cache_layout() {
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let cache = tmp.path();
        // <cache>/models--Org--repo/snapshots/<commit>/onnx/model.onnx
        let revision = "deadbeef";
        let snap = cache
            .join("models--Org--repo")
            .join("snapshots")
            .join(revision);
        fs::create_dir_all(snap.join("onnx")).expect("mkdir");
        let weights = snap.join("onnx/model.onnx");
        fs::write(&weights, b"hello").expect("write weights");

        let found = locate_weight(cache, "Org/repo", revision, "onnx/model.onnx").expect("located");
        assert_eq!(found, weights);
        // Wrong repo / file → not found.
        assert!(locate_weight(cache, "Org/other", revision, "onnx/model.onnx").is_none());
        assert!(locate_weight(cache, "Org/repo", revision, "onnx/missing.onnx").is_none());
        assert!(locate_weight(cache, "Org/repo", "other", "onnx/model.onnx").is_none());
    }

    #[test]
    fn verify_with_enforces_pinned_digest_against_cache() {
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let cache = tmp.path();
        let model_id = "all-MiniLM-L6-v2-Q";
        let revision = pinned_revision(model_id).expect("model must have a pinned revision");
        let repo_dir = cache.join("models--Org--repo");
        fs::create_dir_all(repo_dir.join("refs")).expect("refs dir");
        fs::write(repo_dir.join("refs/main"), revision).expect("write main ref");
        let snap = repo_dir.join("snapshots").join(revision);
        fs::create_dir_all(snap.join("onnx")).expect("mkdir");
        fs::write(snap.join("onnx/model.onnx"), b"hello").expect("write");

        // Pinned digest matches the cached bytes → Verified.
        assert_eq!(
            verify_with(
                Some(HELLO_SHA256),
                model_id,
                cache,
                "Org/repo",
                "onnx/model.onnx"
            )
            .expect("matching digest must verify"),
            IntegrityOutcome::Verified
        );
        // Pinned digest does NOT match → fail-closed Integrity error.
        assert!(
            matches!(
                verify_with(Some("00"), model_id, cache, "Org/repo", "onnx/model.onnx"),
                Err(EmbeddingError::Integrity(_))
            ),
            "a pinned digest that does not match the cached weights must fail closed"
        );
        // Digest pinned but file absent → Integrity error (cannot verify it).
        assert!(
            matches!(
                verify_with(
                    Some(HELLO_SHA256),
                    model_id,
                    cache,
                    "Org/missing",
                    "onnx/model.onnx"
                ),
                Err(EmbeddingError::Integrity(_))
            ),
            "a registered model whose weights are absent must fail closed"
        );
        // No digest pinned → Skipped (dormant), even with no file present.
        assert_eq!(
            verify_with(None, "m", cache, "Org/missing", "onnx/model.onnx").expect("skip"),
            IntegrityOutcome::Skipped
        );
    }

    #[test]
    fn verify_with_fails_closed_when_pinned_snapshot_is_missing() {
        use std::fs;
        let model_id = "all-MiniLM-L6-v2-Q";
        let pinned = pinned_revision(model_id).expect("model must have a pinned revision");
        let other_commit = "0000000000000000000000000000000000000000";
        assert_ne!(other_commit, pinned);

        let tmp = tempfile::tempdir().expect("tempdir");
        let cache = tmp.path();
        let repo_dir = cache.join("models--Org--repo");
        fs::create_dir_all(repo_dir.join("refs")).expect("refs dir");
        fs::write(repo_dir.join("refs/main"), other_commit).expect("write main ref");
        let snap = repo_dir.join("snapshots").join(other_commit);
        fs::create_dir_all(snap.join("onnx")).expect("mkdir");
        fs::write(snap.join("onnx/model.onnx"), b"hello").expect("write");

        let err = verify_with(
            Some(HELLO_SHA256),
            model_id,
            cache,
            "Org/repo",
            "onnx/model.onnx",
        )
        .expect_err("a matching non-pinned snapshot must not verify");
        assert!(
            matches!(
                err,
                EmbeddingError::Integrity(ref msg)
                    if msg.contains(pinned) && msg.contains(other_commit)
            ),
            "error should name loaded commit {other_commit} and pinned commit {pinned}: {err}"
        );
    }

    #[test]
    fn registry_lookups_are_consistent() {
        // The default model is now ENFORCED (#7102 back-fill). An unknown /
        // future model id stays dormant (None) and is accepted with a warning.
        assert!(
            expected_digest("all-MiniLM-L6-v2-Q").is_some(),
            "default model digest must be enforced (#7102 back-fill)"
        );
        assert!(pinned_revision("all-MiniLM-L6-v2-Q").is_some());
        assert!(expected_digest("some-unsupported-model").is_none());
        assert!(pinned_revision("some-unsupported-model").is_none());
        // Every pinned digest MUST carry a provenance revision (guards a
        // back-fill that adds a digest without recording its commit), and the
        // two tables must cover exactly the same set of model ids.
        for (model_id, _digest) in WEIGHT_DIGESTS {
            assert!(
                pinned_revision(model_id).is_some(),
                "digest for {model_id} must record a pinned_revision (#7102 provenance)"
            );
        }
        for (model_id, _commit) in PINNED_REVISIONS {
            assert!(
                expected_digest(model_id).is_some(),
                "pinned_revision for {model_id} must have a matching digest (#7102)"
            );
        }
    }

    #[test]
    fn pinned_tables_are_well_formed() {
        // Back-fill landed (#7102): the enforced allowlist must be non-empty and
        // every entry must be a syntactically valid digest/commit. This guards
        // against truncated, uppercase, or wrong-length values that would make a
        // model fail closed against its own real weights.
        assert!(
            !WEIGHT_DIGESTS.is_empty(),
            "WEIGHT_DIGESTS must be back-filled (#7102)"
        );
        let is_lower_hex = |s: &str| s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        for (model_id, digest) in WEIGHT_DIGESTS {
            // Lowercase-hex SHA-256 is exactly 64 hex chars.
            assert_eq!(
                digest.len(),
                64,
                "digest for {model_id} must be 64 lowercase-hex chars"
            );
            assert!(
                is_lower_hex(digest),
                "digest for {model_id} must be lowercase hex"
            );
        }
        for (model_id, commit) in PINNED_REVISIONS {
            // A Hugging Face commit is a 40-char git SHA-1.
            assert_eq!(
                commit.len(),
                40,
                "commit for {model_id} must be a 40-char git SHA-1"
            );
            assert!(
                is_lower_hex(commit),
                "commit for {model_id} must be lowercase hex"
            );
        }
    }

    #[test]
    fn enforced_default_digest_rejects_mismatch_and_accepts_match() {
        // Pin the captured default digest/commit as a regression guard so an
        // accidental edit is caught, then prove the fail-closed mechanic with
        // the REAL value. (End-to-end acceptance of the multi-MB real weights is
        // covered by the network-gated ignored tests in lib_tests.rs.)
        let expected = expected_digest("all-MiniLM-L6-v2-Q")
            .expect("default model digest must be enforced (#7102)");
        assert_eq!(
            expected,
            "afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1"
        );
        assert_eq!(
            pinned_revision("all-MiniLM-L6-v2-Q"),
            Some("751bff37182d3f1213fa05d7196b954e230abad9")
        );
        // Any bytes that are not the pinned weights fail closed against the real
        // digest — the core supply-chain control, exercised with a real value.
        let err =
            check_digest("all-MiniLM-L6-v2-Q", expected, b"not the pinned ONNX graph").unwrap_err();
        assert!(
            matches!(err, EmbeddingError::Integrity(ref msg) if msg.contains("SHA-256 mismatch")),
            "tampered weights must be rejected against the real pinned digest: {err}"
        );
        // The comparison accepts exactly the bytes that hash to the expected
        // digest (mechanic check with a self-contained vector).
        let probe = b"arbitrary self-contained vector";
        check_digest("probe", &sha256_hex(probe), probe)
            .expect("bytes whose SHA-256 equals the expected digest must be accepted");
    }
}
