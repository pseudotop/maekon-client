# Vendored Tauri-stack fork: delta manifest + refresh process

`Cargo.toml` `[patch.crates-io]` overrides 8 crates with local vendored source
under this directory so the Linux build can use GTK4/WebKitGTK 6 instead of
the upstream GTK3/`webkit2gtk` line. This document is the delta manifest:
what each crate's local source diverges from its pristine crates.io release,
why, and how to refresh a crate to a newer upstream version without losing
the local changes.

Each crate directory also carries a machine-generated `UPSTREAM-DELTA.patch`
(pristine crates.io tarball -> vendored source, unified diff). Treat that
file as the ground truth for "what changed"; this README is the human-readable
"why" and "how to refresh" companion.

## Why this fork exists

Origin: [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html)
(`glib` 0.18.5 unsoundness) plus upstream `webkit2gtk`/GTK3 end-of-life
pressure on Linux. Issue #4230 spiked a manifest-only `webkit2gtk` ->
`webkit6` swap and found it insufficient: `glib 0.18.x` still resolved
through the broader GTK3 stack (`tao`, `tray-icon`/`libappindicator`, `muda`,
`rfd`). Epic "E31" (closed, tracked as `[EPIC E31] Maekon Linux
GTK4/WebKitGTK 6 migration`) scoped the full source-level migration:

1. Owner audit of every GTK3/`glib 0.18` dependency route on the Linux
   target graph.
2. A decision to fork/patch `tao` first (it owns the normal Linux window and
   event loop; a Linux tray/menu/dialog-only fallback could not close the
   `glib 0.18` route because a normal Tauri window still needs `tao`).
3. A source patch spike across `tao`, `wry`, `tauri`, `tauri-runtime`,
   `tauri-runtime-wry`, `tray-icon`, and `muda` to replace GTK3 APIs
   (`gtk::Container`, `pack_start`, `connect_button_press_event`,
   `WindowEdge`, `webkit2gtk::WebView`, etc.) with GTK4/WebKitGTK 6
   equivalents (`webkit6`, `gtk4` under the `gtk` Cargo package rename,
   `GestureClick`, `SurfaceEdge`, etc.).
4. A Linux runner compile pass (`gtk4 4.6.9`, `webkitgtk-6.0 2.50.4`,
   `javascriptcoregtk-6.0 2.50.4`, `libsoup-3.0 3.0.7`) that returned
   `exit_code=0`, after which the epic was closed as production-promoted.

No upstream `tauri-apps/{tao,wry,tauri,muda,tray-icon}` GitHub issue number
is referenced anywhere in the vendored source comments or the local review
docs — this fork was developed independently in response to the RUSTSEC
advisory and webkit2gtk EOL pressure, not by filing or following an upstream
migration issue. If an upstream issue is opened later, record it in the
"Upstream tracking" column below.

Local evidence trail (internal engineering-review documents, not part of
this public export — filenames listed here only for parent-repo
traceability; most recent supersedes the earlier ones):
`2026-06-07-webkitgtk6-spike.md` (#4230),
`2026-06-07-e31-linux-gtk3-owner-audit.md` (#5410),
`2026-06-07-e31-gtk4-path-decision.md` (#5411),
`2026-06-07-e31-tao-gtk4-source-spike.md` (#5414),
`2026-06-07-e31-tao-gtk4-linux-runner-capture.md` (#5416, real Linux
compile evidence landed via PR #5409, referenced from the epic's closing
comment — the doc file itself predates that comment and was not
updated afterward, so read the epic issue thread for the final result,
not just the doc).

## Per-crate manifest

`Delta` = added+removed lines in `UPSTREAM-DELTA.patch` (pristine
crates.io tarball vs. vendored source, noise-excluded — see "Exclusions"
below). `Files` = number of files with at least one differing line.
Measured directly against `static.crates.io` tarballs for the exact pinned
version; regenerate before trusting these numbers stale by more than one
refresh cycle.

| Crate dir | Upstream version | Delta (files / lines) | Vendor commit | Modification commits | Why |
| --- | --- | --- | --- | --- | --- |
| `tao-0.35.2` | `tao` 0.35.2 | 10 files / ~5306 lines | `a94c1f3d84` | `b2155a581f`, `6c070616cc`, `a37c2a4b92`, `a69dcad3bd`, `ff5104aa9c`, `7e681d6ac2`, `43b9458ea4` | Primary hard blocker. Owns the normal Linux window/event loop; must move off `gtk 0.18`/`gdk*-sys 0.18` (`v3_24` features) to `gtk4`/`v4_6` before anything downstream can compile against GTK4. Largest and highest-risk delta: `Cargo.toml` Linux target deps, `platform_impl/linux/{window,event_loop,monitor,util}.rs`, `platform_impl/linux/wayland/header.rs`, `platform/unix.rs` public extension surface, `device.rs`, `keyboard.rs`, `portal.rs`. |
| `wry-0.55.1` | `wry` 0.55.1 | 8 files / ~667 lines | `a94c1f3d84` | `b2155a581f`, `1f91b427b7`, `6f4d27dd2c` | WebKitGTK webview integration. `src/webkitgtk/*` migrated from `webkit2gtk::WebView` to `webkit6::WebView`; `src/webkitgtk/drag_drop.rs` disables the GTK3 drag-drop signal handler pending a `DropTarget`-based port (only 1 line carries an explicit `E31` comment marker in this crate). |
| `tauri-runtime-wry-2.11.2` | `tauri-runtime-wry` 2.11.2 | 5 files / ~178 lines | `a94c1f3d84` | `6c070616cc`, `9cf4a7ec0a`, `9e5afba10f` | Bridges Tao + Wry into the Tauri runtime. `Cargo.toml` gtk4/webkit6 deps, `src/lib.rs` `gtk::glib::IsA` -> `gtk::prelude::IsA`, `src/monitor/linux.rs` `workarea()` -> `geometry()` fallback (GTK4 removed monitor workarea), `src/undecorated_resizing.rs` full GTK3 `connect_button_press_event`/`WindowEdge` rewrite to GTK4 `GestureClick`/`SurfaceEdge`, `src/webview.rs` `webkit2gtk::WebView` -> `webkit6::WebView` type alias. **Correction**: issue #7722's originating review cited this crate's delta as "4 lines"; the actual pristine-vs-vendored diff measured for this manifest is ~178 lines across 5 files. Still comparatively small next to `tao`/`muda`/`tray-icon`, but not 4 lines — use the measured `UPSTREAM-DELTA.patch` as the source of truth. |
| `tauri-2.11.2` | `tauri` 2.11.2 | 11 files / ~115 lines | `a94c1f3d84` | `b2155a581f`, `6c070616cc`, `9cf4a7ec0a`, `81d0a00a97` | Root feature aggregation. `Cargo.toml` `wry` feature list `webkit2gtk` -> `webkit6`, gtk4 deps, `muda` feature list drops `"gtk"` (Linux menu-to-GTK bridging removed — see `menu/menu.rs`, `menu/submenu.rs`, `manager/menu.rs`, `window/mod.rs`, all gated to `#[cfg(any(target_os = "macos", target_os = "windows"))]` on the GTK-menu-attachment call sites, i.e. Linux native menu attachment is now a documented no-op pending a GTK4 replacement). Doc-comment link updates (`webkit2gtk` -> `webkit6` URLs) and a `mobile/ios-api/Sources/Tauri/Logger.swift` whitespace-only reformat are also present (unrelated to E31; likely an editor/format pass caught in the same vendor commit). |
| `muda-0.19.1` | `muda` 0.19.1 | 11 files / ~2053 lines | `a94c1f3d84` | `7ef51aa433`, `81d0a00a97` | Menu backend for Tauri/tray-icon. `src/platform_impl/gtk/*` (the GTK3 menu implementation) is gutted and replaced by a new `src/platform_impl/fallback/mod.rs` (+265 lines) that returns an in-process no-op/fallback menu on Linux — this is the "no-gtk linux muda fallback" referenced in commit `7ef51aa433`, not a GTK4 port. Large removed-line count (~1802 of the ~2053 total) reflects deleting the GTK3 implementation rather than porting it. |
| `tray-icon-0.23.1` | `tray-icon` 0.23.1 | 7 files / ~1202 lines | `b2155a581f` (vendor + first modification are the **same commit** — see "Known issues") | `833d4e02d3` | Tray icon backend for Tauri. Same pattern as `muda`: `src/platform_impl/gtk/*` GTK3/`libappindicator` backend is disabled via a new `src/platform_impl/disabled.rs` that returns an unsupported-platform error on Linux/BSD, keeping Windows/macOS tray support intact. This crate carries the most explicit self-documentation of any vendored crate: its own `README.md` and `src/lib.rs` have 5 inline "Maekon E31 patch note" comments explaining the Linux/BSD disablement — read those directly for the fullest first-party rationale. |
| `tauri-runtime-2.11.2` | `tauri-runtime` 2.11.2 | 3 files / ~18 lines | `a94c1f3d84` | `6c070616cc`, `9cf4a7ec0a` | Smallest real delta. `Cargo.toml` gtk4/webkit6 deps, `src/webview.rs` and `src/window.rs` `webkit2gtk::WebView`/`gtk::glib::IsA` type/trait-path updates to match the GTK4-clean Wry/Tao types flowing through this crate's trait definitions. |
| `tauri-plugin-webdriver-0.2.1` | `tauri-plugin-webdriver` 0.2.1 | 3 files / ~23 lines | `a94c1f3d84` (vendor + modification are the **same commit** — see "Known issues") | none | **Correction**: issue #7722's originating review characterized this crate's delta as "0 lines" (implying a pure, unmodified vendor copy). The actual pristine-vs-vendored diff is not empty: `Cargo.toml` swaps `cairo-rs 0.18`/`glib 0.21`/`gtk 0.18`/`javascriptcore-rs 1.1`/`webkit2gtk 2.0` for `cairo-rs 0.21`/`glib 0.22`/`gtk4 0.11`/`javascriptcore6 0.6`/`webkit6 0.6`, and `src/platform/linux.rs` renames 3 `webkit2gtk::*` imports/calls to `webkit6::*`. This matches the same GTK4/WebKitGTK 6 migration pattern as the other 7 crates and is attributable to the original `#4230` WebKitGTK 6 manifest spike (its evidence doc explicitly mentions patching "the optional `tauri-plugin-webdriver` lockfile path"), even though no later `fix:` commit ever touched it separately. No `UPSTREAM-DELTA.patch` empty-marker was needed; the real 23-line delta is committed. |

## Exclusions

`UPSTREAM-DELTA.patch` is generated with:

```bash
diff -ruN --exclude=target --exclude=.cargo_vcs_info.json \
  --exclude=.cargo-ok --exclude=Cargo.toml.orig --exclude=Cargo.lock \
  <pristine-crate-dir> <vendored-crate-dir>
```

Excluded paths and why:

- `target/` — build output, never present in either tree in practice, excluded defensively.
- `.cargo_vcs_info.json`, `.cargo-ok` — crates.io packaging metadata written by `cargo package`/`cargo publish`; not present in the vendored tree at all, would otherwise show as a spurious whole-file deletion in every crate's delta.
- `Cargo.toml.orig` — crates.io's saved copy of the pre-`cargo package`-normalization manifest; same as above, not part of the vendored tree.
- `Cargo.lock` — every pristine tarball ships a `Cargo.lock` (these are all binary-adjacent/example-bearing crates); the vendored copies do not ship one. Per-patch-crate lockfiles were deliberately removed as non-shipping Dependabot noise (commit `1a4a0a85f0`, PR #5862, part of the `#5435` rc.6 release gate). Excluding it keeps the delta focused on the actual source/manifest modifications that matter for a refresh; the removal itself is already documented by that commit.

If you regenerate a delta and it includes any of the above, you excluded
the wrong flag set — re-check the command above.

## Known issues

### `tauri-plugin-webdriver` MSRV exceeds workspace MSRV

`patches/tauri-plugin-webdriver-0.2.1/Cargo.toml` declares
`rust-version = "1.90"`. The workspace MSRV (`clients/maekon-client/Cargo.toml`
`[workspace.package] rust-version`) is `1.88.0`. Every other vendored crate
declares a `rust-version` at or below the workspace floor (`tao` 1.74,
`muda`/`tray-icon` 1.73, `wry` 1.77, `tauri`/`tauri-runtime`/
`tauri-runtime-wry` 1.77.2), so `tauri-plugin-webdriver` is the sole
outlier.

This is flagged here rather than fixed in place because lowering
`rust-version` in `patches/tauri-plugin-webdriver-0.2.1/Cargo.toml` is a
source change to vendored code, and this manifest/process change is scoped
to be a zero-functional-diff documentation change. Fix it at the next
refresh of this crate (see "Refresh procedure" below): when regenerating
`patches/tauri-plugin-webdriver-0.2.1/Cargo.toml`, either confirm the
declared floor is still accurate against the new pristine `Cargo.toml`, or
add an explicit local `rust-version` correction as its own commit (per
step 3's "separate commits" rule below) with an inline comment explaining
the divergence from upstream.
Until then, this is a latent MSRV-policy violation that `cargo` will not
surface unless something actually requires Rust 1.89 or 1.90 features —
track it so it does not silently regress the workspace's declared MSRV
guarantee.

### Two crates have no clean pristine-vendor commit boundary

For `tray-icon-0.23.1` and `tauri-plugin-webdriver-0.2.1`, the initial
vendor (unpacking the pristine crates.io source into `patches/`) and the
first source modification landed in the same commit (`b2155a581f` and
`a94c1f3d84` respectively). For the other 6 crates, the vendor commit
(`a94c1f3d84`) is a clean, unmodified copy of the pristine crate, and every
GTK4 source change is a separate later `fix:` commit. This means git
history alone cannot reconstruct a pristine baseline for those 2 crates —
which is exactly the gap `UPSTREAM-DELTA.patch` now closes going forward:
regardless of commit granularity, the pristine baseline is always one
`curl`+`tar` away, not an archaeology exercise.

## Refresh procedure

Follow this whenever a CVE/security fix, or a routine version bump, needs
to land in one of these 8 crates. Do this per-crate, not as a batch — each
crate's modifications are independent and a batch refresh makes review and
rollback harder.

1. **Download pristine.** Fetch the new version's tarball directly from the
   immutable crates.io CDN (do not use the registry API JSON endpoint, which
   is rate-limited for automated tooling):

   ```bash
   curl -L https://static.crates.io/crates/<name>/<name>-<new-version>.crate \
     -o /tmp/<name>-<new-version>.crate
   mkdir -p /tmp/pristine && tar xzf /tmp/<name>-<new-version>.crate -C /tmp/pristine
   ```

2. **Apply `UPSTREAM-DELTA.patch` (3-way).** Copy the pristine source into
   `patches/<name>-<new-version>/`, then apply the existing delta on top of
   it with 3-way merge so conflicting hunks (upstream moved/changed the same
   lines Maekon patched) surface as conflict markers instead of failing
   silently:

   ```bash
   cp -r /tmp/pristine/<name>-<new-version> patches/<name>-<new-version>
   git -C patches/<name>-<new-version> init -q  # only if you want `git apply --3way`; otherwise use `patch`
   patch -p1 -d patches/<name>-<new-version> < patches/<old-name>-<old-version>/UPSTREAM-DELTA.patch
   # or, for a real 3-way merge with conflict markers:
   #   git apply --3way -p1 --directory=patches/<name>-<new-version> \
   #     patches/<old-name>-<old-version>/UPSTREAM-DELTA.patch
   ```

   Resolve any conflicts by hand, consulting the "why" column above for the
   intent of each hunk (most hunks are GTK3->GTK4/WebKitGTK6 API renames —
   if upstream itself ships GTK4 support by the time you do this, most or
   all hunks should simply stop applying because the upstream code already
   matches the target state; see "Exit criteria" below).

3. **Commit pristine-vendor and re-applied modifications as separate
   commits.** This preserves the boundary that `tao`, `wry`, `tauri`,
   `tauri-runtime`, and `tauri-runtime-wry` already have and that
   `tray-icon`/`tauri-plugin-webdriver` are missing:
   - Commit 1: `chore(maekon-client): vendor <name> <new-version> pristine source` — the untouched pristine copy, no local modifications.
   - Commit 2+: `fix(maekon-client): port E31 GTK4 patch to <name> <new-version>` (split further if the conflict resolution touches unrelated concerns) — the re-applied/hand-resolved local modifications.
   - Remove the old `patches/<old-name>-<old-version>/` directory and update
     `Cargo.toml` `[patch.crates-io]` to point at the new path, in the same
     change set as commit 2 (so `cargo metadata` never points at a stale
     path mid-history).

4. **Regenerate `UPSTREAM-DELTA.patch`.**

   ```bash
   diff -ruN --exclude=target --exclude=.cargo_vcs_info.json \
     --exclude=.cargo-ok --exclude=Cargo.toml.orig --exclude=Cargo.lock \
     /tmp/pristine/<name>-<new-version> patches/<name>-<new-version> \
     > patches/<name>-<new-version>/UPSTREAM-DELTA.patch
   ```

   Strip the embedded `---`/`+++` timestamps before committing (they are
   non-reproducible local mtimes and add pure diff-noise on every
   regeneration):

   ```bash
   sed -i.bak -E 's/\t[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?( [+-][0-9]{4})?$//' \
     patches/<name>-<new-version>/UPSTREAM-DELTA.patch
   rm patches/<name>-<new-version>/UPSTREAM-DELTA.patch.bak
   ```

   Update this table's `Upstream version` / `Delta` / `Vendor commit` /
   `Modification commits` columns for the refreshed crate in the same
   change set.

5. **Run the audit script + full CI.**

   ```bash
   bash scripts/audit-e31-experimental-patches.sh
   ```

   This must report `Promotion blockers: 0` — it fails closed on any
   reintroduced `webkit2gtk`, GTK3 container/window/event API, GTK3 Tao
   geometry/WM API, GTK3 Wry geometry API, `glib 0.18`, or missing-Tao-patch
   condition. Then run the full workspace CI lane
   (`maekon-client-private-tests.yml`, `workflow_dispatch`) including the
   `e31_linux_capture_only` input if the refresh touched `tao`, `wry`,
   `tauri-runtime-wry`, or `tauri`, to get real Linux GTK4/WebKitGTK6
   compile evidence before merging — do not merge a refresh on macOS-only
   `cargo check` evidence per the closure criteria recorded in the internal
   `2026-06-07-e31-gtk4-path-decision.md` review doc (#5411, see "Local
   evidence trail" above).

6. **Windows crate convergence (#7743, ctd-W3 A3).** Re-verify the `windows`/
   `windows-sys` version split against the pristine tarball's `Cargo.toml`
   and this workspace's `Cargo.lock`, and align them where the refresh makes
   that possible:

   ```bash
   grep -n '^name = "windows"$' -A2 Cargo.lock
   grep -n '^name = "windows-sys"$' -A2 Cargo.lock
   cargo tree --target x86_64-pc-windows-msvc --features grpc,windows-sandbox -i windows
   ```

   Measured at the time this step was written (Cargo.lock, this workspace):
   `windows` has 2 coexisting versions — `0.61.3` (pulled by the vendored
   `patches/{tao,tauri,tauri-runtime,tauri-runtime-wry}` family AND,
   independently, by `enigo` — `maekon-automation`'s plain crates.io
   dependency, NOT part of this patch stack) and `0.62.2` (pulled by `cpal`,
   `maekon-vision`, `sysinfo`, `xcap` — all plain workspace/registry deps,
   none of them vendored). `windows-sys` is more fragmented still: 5
   coexisting versions (`0.45.0`, `0.52.0`, `0.59.0`, `0.60.2`, `0.61.2`),
   pulled by `self-replace`, `global-hotkey`, `window-vibrancy` (the latter
   via `patches/tauri-2.11.2`), `keyring`, and `clap`'s `anstyle-query`
   transitive chain — only ONE of those five consumers (`window-vibrancy`,
   reached through the vendored `tauri` patch) is inside this fork's control
   at all.

   **What a refresh CAN fix**: if the new pristine `tao`/`wry`/`tauri`/
   `tauri-runtime`/`tauri-runtime-wry` release itself bumped its `windows`/
   `windows-sys` requirement (check the pristine `Cargo.toml` you downloaded
   in step 1 against the vendored copy's current one), let that bump flow
   through naturally — do not pin an older `windows`/`windows-sys` version in
   the vendored manifest than pristine upstream now declares. If, after the
   refresh, `cargo tree -i windows` still shows 2+ versions purely because
   OTHER workspace dependencies (`enigo`, `cpal`, `sysinfo`, `xcap`,
   `self-replace`, `global-hotkey`, `keyring`, `clap`) haven't converged
   themselves, that is expected and NOT a fork-refresh blocker — those are
   plain crates.io deps `cargo update`/Dependabot already manage
   independently of this vendored stack.

   **What a refresh CANNOT fix**: this fork's Cargo.toml pins are frozen at
   whatever the vendored crate release declared; a fork refresh cannot force
   an unrelated dependency (`enigo`, `keyring`, etc.) to adopt a newer
   `windows`/`windows-sys` line — that only happens when THAT crate's own
   upstream releases a version pinning it, tracked by the normal Dependabot
   queue, not this procedure.

   Record the before/after `windows`/`windows-sys` version counts in the
   refresh's commit message or PR description so the next refresh has an
   accurate baseline instead of re-deriving it from scratch.

## Weekly audit lane

`scripts/audit-e31-experimental-patches.sh` is cheap (ripgrep pattern
matching over `patches/` plus a `Cargo.lock` scan; no `cargo` invocation, no
GTK/WebKitGTK system packages required) and now runs on a weekly schedule
in `.github/workflows/maekon-client-patch-audit.yml` (workflow display name
`Maekon Client Weekly Checks`), in addition to its existing
`workflow_dispatch` invocation in `maekon-client-private-tests.yml`'s
`e31_linux_capture_only` lane (which bundles it with a full Linux
GTK4/WebKitGTK6 `cargo check`, too expensive to run unattended on a schedule
per the repository's CI-cost-minimization policy). The weekly lane
catches silent drift — e.g. a future dependency bump anywhere in the
workspace pulling `glib 0.18` back into the Linux target graph through an
unrelated path — between the infrequent manual `workflow_dispatch` runs.

The same workflow's second job, `feature-cell-check` (#7732 ctd-W2 D2),
carries a `cargo check`-only weekly matrix for the vendored
`tauri-plugin-webdriver` patch (`maekon-app --features webdriver`) alongside
7 other feature cells (`maekon-vision`'s `ocr`/`ml-detect`,
`maekon-analysis`'s `hnsw`, and `maekon-app`'s `audio`/`stt`/`download`/
`embedding`) that no CI job in this parent monorepo compiled before this
lane existed. It is grouped with the patch-audit job here because both are
cheap, unattended, weekly, compile/text-health checks over this same
vendored-patch-adjacent surface — see the workflow file's own header comment
for the full per-cell rationale and apt-package sourcing.

The same workflow's THIRD job, `first-party-lint-per-os` (#7743 ctd-W3 A2c),
runs `cargo clippy --workspace --all-targets --features grpc -- -D warnings`
(the identical invocation the public-export `check` job runs on
ubuntu-latest) on `macos-latest` and `windows-latest`. This is the ONE job in
this trio that is NOT cheap — a cold Tauri + vendored-fork build on those
runners is materially slower than the other two jobs' text/`cargo check`-only
work — but it closes a real gap: no CI in this parent monorepo previously
deny-linted first-party `#[cfg(target_os = "macos"/"windows")]` code (the
public `ci.yml`'s own `check` job clippy steps are ubuntu-only and never run
in this monorepo anyway; this monorepo's `test-platform`-equivalent coverage
only ever EXECUTES platform tests under a relaxed `RUSTFLAGS: -W warnings`,
deliberately, because the vendored `patches/` fork is an uncapped
path-dependency stack that a workspace-global `-D warnings` would also lint —
see `Cargo.toml`'s `[workspace.lints.rust]` comment and this crate's
`.github/workflows/ci.yml` top-of-file `RUSTFLAGS` comment for the full
empirically-verified mechanism writeup). Because it is expensive, it stays
weekly rather than per-PR, matching this workflow's existing cadence and the
repository's Actions-cost policy.

## Exit criteria — when to drop this fork

Drop the vendored patches and go back to plain upstream crates.io
dependencies for all 8 crates once upstream `tauri-apps/tao`,
`tauri-apps/wry`, `tauri-apps/tauri` (which pulls in `tauri-runtime` and
`tauri-runtime-wry`), `tauri-apps/muda`, and `tauri-apps/tray-icon` ship
native GTK4/WebKitGTK 6 support on their default/stable release line (not
just an opt-in feature flag still defaulting to GTK3). At that point:

1. Re-run the refresh procedure above against the new upstream version.
2. If the regenerated `UPSTREAM-DELTA.patch` for a crate comes back empty
   (or only contains packaging-noise hunks already covered by
   "Exclusions"), that crate no longer needs a local patch — remove its
   `[patch.crates-io]` entry and delete `patches/<name>-<version>/`
   entirely instead of re-vendoring it.
3. Once all 8 entries are removed, `bash scripts/audit-e31-experimental-patches.sh`
   should report "No experimental patch root present" and can itself be
   retired (or kept as a permanent regression guard against a future GTK3
   dependency creeping back in).

See the internal `2026-06-07-e31-gtk4-path-decision.md` review doc's
"Closure Criteria" section (#5411, see "Local evidence trail" above) for
the original acceptance bar this fork was built against, and
the closing comment on the epic issue (`[EPIC E31] Maekon Linux
GTK4/WebKitGTK 6 migration`) for the real Linux runner compile evidence
(`gtk4 4.6.9`, `webkitgtk-6.0 2.50.4`, `exit_code=0`) that promoted this
fork from experimental spike to the production `[patch.crates-io]` wiring
it has today.
