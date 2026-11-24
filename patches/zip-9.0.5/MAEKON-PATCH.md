# ZIP 9.0.5 local security fixes (#13038)

Baseline: crates.io `zip` 9.0.5, archive SHA-256
`6e04976da629d3b04204ba8105b396a193af8ad2583b2c79bfb3ab8af580a11e`,
upstream commit `0123ed144cfa7d672dae78b79a034d8900f9aaeb`.
All 47 published production Rust files matched that commit before patching.
`UPSTREAM-DELTA.patch` records the exact local source/manifest changes. Packaging
metadata `.cargo_vcs_info.json`, `Cargo.toml.orig` and nested `Cargo.lock` are
excluded using the parent patch convention (`.cargo-ok` is absent in the archive).

## Why the published version is insufficient

ZIP 8.6.0 is affected by RUSTSEC-2026-0367, RUSTSEC-2026-0368 and
RUSTSEC-2026-0369. ZIP 9 includes the fixes, including Windows verbatim path
validation, but 9.0.5 still scans and caches payload signatures before ordinary
metadata reads. An 8 MiB payload produced 33,555,249 bytes of extra allocation
and 8,414,427 bytes of reads, compared with 559 bytes and 1,097 reads in 8.6.
Dropping the new caches revives the old superlinear EOCD attack, so this fork
retains their defensive behavior and bounds exceptional fallback work.

## Local delta

- Check the expected central-directory/ZIP64 EOCD position before scanning.
  Cache at most 65,536 offsets per signature kind (512 KiB per vector), rejecting
  unusually ambiguous input with UnsupportedArchive.
- Preserve an unrestricted first central-directory candidate. After a failed
  candidate, cap cumulative newly parsed central headers at 8,192. This bounds
  overlapping suffix caches at different directory offsets.
- Count actual ZIP64 candidate reads, including the extensible sector, across
  outer EOCD retries. Stop new candidates after `4 * file_len + 1024` bytes.
  Completing the current candidate can add at most one file length. This is a
  metadata fallback bound; it does not replace callers' decompressed-size caps.
- Return zero for an empty AES read without treating it as EOF or finalizing the
  authentication state. Subsequent nonempty reads still authenticate the MAC.
- Include archive comments when detecting trailing bytes after `abort_file()`;
  return the relocated central-directory offset to `finish_into_readable()`.
- Replace the sole nix use on macOS/FreeBSD/OpenBSD/NetBSD/DragonFly with the same
  libc fchmodat call, identical target cfg, permission-bit mask, NUL rejection,
  errno conversion and AT_SYMLINK_NOFOLLOW. This removes a new broad dependency
  without narrowing supported targets or codecs. The existing libc 0.2.190
  graph supplies the platform constants and ABI.
- Test-only (#8685): `aes::tests::test_aes_password_verification_constant_time`
  and `write::tests::{remove_encrypted_file, remove_encrypted_aligned_symlink,
  test_fuzz_failure_2024_05_08}` use a per-run password instead of a literal,
  which CodeQL reported as a hard-coded credential. The AES test still checks
  wrong passwords that differ from the correct one only at byte 0 and at byte 1;
  its salt was already random per run. No library code changes.

The workspace retains default codec features and enables `unreserved` so raw
copying an unchanged XLSX member retains its UID/GID (0x7875) extra field.

## Acceptance boundary

A first valid directory with 65,537 entries and a valid ZIP64 with a 1 MiB
extensible sector remain accepted. Once a malformed EOCD candidate has been
seen, the 8,192-header cumulative fallback bound can reject a later genuine
large directory, including one behind a fake EOCD in a ZIP comment. Likewise,
prefixed archives with more than 65,536 apparent signatures can be rejected.
These are deliberate limits on ambiguous offset detection, not a promise to
accept every syntactically valid archive. TMD's normal 2,048-entry limit is
stricter than this fallback limit.

## Review scope and validation

The ZIP 8.6 -> 9.0.5 source delta was reviewed for packed Pod layouts, checked
metadata offsets, bounded extra fields, incremental legacy decoders, declared
size/CRC/AES enforcement, Windows/Unix path handling, symlink resolution and
writer/raw-copy semantics. The local delta received independent source review.
This does not certify the defective published 9.0.5 version as safe-to-deploy.
The path policy identifies reviewed local source; it is not a crates.io audit.

The repository integration test `zip_archive_resource_limits` preserves the
complete upstream 321-case EOCD complexity matrix and covers the ordinary and
adversarial allocation/read budgets, large first-valid ZIP64 archives,
abort/comment/readback/append, empty AES reads plus tampered MAC rejection, and
symlink target protection. A separate deferred-permissions fixture queues a
directory, then replaces it with a symlink during the next entry's payload read.
The reader hook is armed only after archive metadata parsing and must fire once.
The resulting helper call must preserve the target's 0600 mode and contents;
on macOS/BSD it must also change the link itself to the requested nondefault
0711 mode. A symlink-only archive does not reach this helper. Linux checks its
no-follow guard, while the libc call requires the admitted native platform run.
Removing individual cache/read limits or upstream
chain reuse made the corresponding resource checks fail during validation. The
allocation meter reports requested live bytes, not RSS, allocator overhead or
temporary old-plus-new storage internal to System.realloc. It establishes these
regressions' requested-allocation bound, not an operating-system memory ceiling.

Exact-commit upstream suites passed 174 library tests with unreserved/legacy-zip,
80 parser/extraction/crypto security tests and 28 further parser/writer tests
(the latter sets overlap only at the repeated EOCD matrix). Optional AES
instruction-count testing failed with EINVAL in the VM and is not claimed as
passing. The POSIX wrapper matched nix over 65,537 modes plus normal, non-UTF8,
NUL, missing, dangling and symlink paths on Linux; both forms and the whole ZIP
candidate cross-compiled for two macOS architectures, FreeBSD and NetBSD. Native
macOS/BSD runtime behavior requires the admitted platform run. No OpenBSD or
DragonFly cross/native run is claimed.

Run the durable application regression from the client workspace:

```sh
cargo test --locked -p maekon-storage --test zip_archive_resource_limits
```

The optional platform workflow runs that same command on native macOS only for
an explicitly admitted `platform-regression` dispatch. Cross compilation does
not stand in for that runtime result.

## Refresh and removal

Refresh from the checksum-verified crate archive with pristine and modification
commits separated. Regenerate the exact delta without metadata or timestamp
noise. Keep the resource/crypto/path tests and all existing supply-chain gates.
Remove the override only when the upstream version passes those tests and its
transitive graph is covered by the existing audit policy. Do not replace this
fork with an advisory ignore, a feature reduction or an unaudited exemption.
