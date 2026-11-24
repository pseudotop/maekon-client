# PPMd 1.5.1 local thread-trait fix (#13038)

Baseline: crates.io `ppmd-rust` 1.5.1, archive SHA-256
`494b4379484fc92217cc7d9e3a532375b1d01a61889ec8333025dc3b0377af56`.
The upstream source commit recorded by the package is
`26f9475c2e33f4b25e6fa8accf6272c972307ac4`.
`UPSTREAM-DELTA.patch` contains the six codec modules, explicit trait-test target
and new test. `.cargo_vcs_info.json`, `Cargo.toml.orig` and nested `Cargo.lock`
use the parent exclusions (`.cargo-ok` is absent in the archive).

## Defect and correction

All six public encoder/decoder types implemented Send and Sync without requiring
those traits on their owned reader/writer. A safe IO type carrying Rc<Cell<_>>
therefore made the codec cross thread boundaries despite exposing that same IO
through get_ref/get_mut/into_inner and Read/Write. This is a real safe-API
soundness defect, present in the earlier 1.4 baseline as well as 1.5.1.

Add independent `Read/Write + Send` and `Read/Write + Sync` bounds to the twelve
unsafe implementations. The codec's model pointers refer into its owned stable
heap, mutable model operations require `&mut self`, and shared access exposes
the reader/writer. Thus Send and Sync require their respective inner-IO trait;
neither requires both traits. No algorithm or codec support is removed.

Each unsafe implementation has an E0277 compile-fail documentation case with a
non-thread-safe inner IO type. `tests/thread_traits.rs` also checks Send-only
(Cell), Sync-only (MutexGuard phantom) and both (Cursor<Vec<u8>>) IO types. The
12 negative cases and positive test passed with default and `unstable-tagged-offsets`
features. Removing only the bounds made all twelve negative cases fail, proving
the regressions exercise the defect. Six upstream library tests and four
security regression tests passed with all features. The large upstream
reference-fixture corpus was not downloaded and is not claimed as executed.

Reproduce the durable trait regressions from the client workspace by copying
the crate outside the workspace. Path overrides are deliberately not workspace
members; invoking Cargo directly on their nested manifests would fail Cargo's
workspace-membership check. This crate has no dependencies. Each copy uses its own target directory so an
experiment with the same package name/version cannot reuse another copy's output.

```sh
trait_dir=$(mktemp -d "${TMPDIR:-/tmp}/maekon-ppmd-traits.XXXXXX")
cp -R patches/ppmd-rust-1.5.1/. "$trait_dir/"
CARGO_TARGET_DIR="$trait_dir/target" cargo test --manifest-path "$trait_dir/Cargo.toml" --doc
CARGO_TARGET_DIR="$trait_dir/target" cargo test --manifest-path "$trait_dir/Cargo.toml" --test thread_traits
CARGO_TARGET_DIR="$trait_dir/target" cargo test --manifest-path "$trait_dir/Cargo.toml" --doc --features unstable-tagged-offsets
CARGO_TARGET_DIR="$trait_dir/target" cargo test --manifest-path "$trait_dir/Cargo.toml" --test thread_traits --features unstable-tagged-offsets
```

## Audit scope and refresh

The existing owner-approved lzma/PPMd publisher policy remains unchanged for the
upstream baseline. The crates.io release metadata independently matches this
archive checksum and publisher user-id 102749 (hasenbanck), published
2026-10-18T13:46:02Z inside the existing 2025-02-28 .. 2027-05-29 window. This
focused trait-bound review is not a new whole-crate unsafe-algorithm audit and does not certify published 1.5.1 as safe-to-deploy.
The path policy denotes the corrected local source with this exact delta.
Retain upstream 1.5.1's earlier heap-bound/minimum-memory fixes; a downgrade to
1.4 is not equivalent. An unreleased larger pointer-model rewrite is not part
of this fix.

On refresh, verify the immutable archive checksum, keep pristine and modification
commits separate, regenerate the delta, and run both negative and independent
positive trait tests. Remove the override after upstream includes equivalent
bounds and the resolved graph passes the unchanged supply-chain gates.
