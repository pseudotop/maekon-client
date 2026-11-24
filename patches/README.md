# Vendored dependency forks: delta manifests and refresh process

`Cargo.toml` `[patch.crates-io]` overrides the active crates in the manifest below
with local vendored source under this directory so the Linux build can use
GTK4/WebKitGTK 6 instead of the upstream GTK3/`webkit2gtk` line. The retired
`tauri-plugin-webdriver-0.2.1` predecessor remains here as historical
provenance but is not wired into the dependency graph. This document is the
delta manifest:
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
| `tao-0.37.1` | `tao` 0.37.1 | 9 files / 1840 lines | #13020 | #13020 (refresh); carried from 0.35.2: `b2155a581f`, `6c070616cc`, `a37c2a4b92`, `a69dcad3bd`, `ff5104aa9c`, `7e681d6ac2`, `43b9458ea4`, #12652 | Primary hard blocker. Owns the normal Linux window/event loop; must move off `gtk 0.18`/`gdk*-sys 0.18` (`v3_24` features) to `gtk4`/`v4_6` before anything downstream can compile against GTK4. Delta: `Cargo.toml` Linux target deps, `platform_impl/linux/{window,event_loop,monitor,util,device,keyboard,portal}.rs`, and the `platform/unix.rs` public extension surface. **#12652**: the motion/click controller closures in `platform_impl/linux/event_loop.rs` capture *weak* window refs. A strong clone owned by the window's own controller is a reference cycle that GTK4 `gtk_window_destroy()` (unlike GTK3 `gtk_widget_destroy()`) never breaks, so `destroy` never fired, `WindowEvent::Destroyed` was never sent and Tauri kept a dead window under its label. **#13020**: the vendored files are kept in upstream's format (`rustfmt --edition 2021 --config tab_spaces=2` reproduces upstream tao exactly); the 0.35.2 delta (~5378 lines) was mostly a 4-space reformat. Ported from 0.36–0.37: `RwLock` for `Window`'s fullscreen, size-constraint and theme state, std `LazyLock` (upstream dropped `once_cell`), JIS key mappings as `gdk::Key` constants, and removal of the Wayland `WlHeader` module (upstream #1218; GTK4's default client-side titlebar follows the window title). Not ported because they are GTK3-only: timestamp de-duplication of mouse signals with `Propagation::Proceed`, the configure-event maximize deferral, and the empty `EventBox` titlebar on undecorated Wayland windows — the GTK4 port uses event controllers, and GTK4 always draws client-side decorations on Wayland. |
| `wry-0.57.0` | `wry` 0.57.0 | 10 files / 787 lines | #13020 | #13020 (refresh); carried from 0.55.1: `b2155a581f`, `1f91b427b7`, `6f4d27dd2c`, #12580, #12652; #8685 | WebKitGTK webview integration. `src/webkitgtk/*` migrated from `webkit2gtk::WebView` to `webkit6::WebView`; both `GtkBox` attach paths set `vexpand`/`hexpand` on the webview to restore the GTK3 port's expand-and-fill box packing, without which the Linux webview is 0 px tall (#12580); `src/webkitgtk/drag_drop.rs` disables the GTK3 drag-drop signal handler pending a `DropTarget`-based port. **#12652**: the synthetic back/forward mouse gesture closures in `src/webkitgtk/synthetic_mouse_events.rs` capture a *weak* webview ref. **#13020**: 0.57's new Linux code is moved to webkit6 — the permission-request handler uses webkit6's request types and `PermissionRequestExt`; display-capture detection calls `webkit6::functions::user_media_permission_is_for_display_device` (upstream gated it behind a `v2_42` feature wry does not define, so it always took the fallback; the patched stack requires WebKitGTK 2.50); the IPC handler reads the JSC value webkit6 passes directly; and the added `glib::{Cast, IsA}` import is dropped because glib 0.22 exports them only from the prelude. **#8685**: the three converters that copy a stored cookie into `cookie::Cookie` (`cookie_from_soup_cookie`, `cookie_from_win32`, `cookie_from_wkwebview` in `src/{webkitgtk,webview2,wkwebview}/mod.rs`) call the `Cookie` setters instead of `CookieBuilder`. Each builder method calls the same setter, so the copied cookie is identical; CodeQL `rust/insecure-cookie` read `CookieBuilder::build()` after the copied, non-literal Secure flag as a newly created insecure cookie. |
| `tauri-runtime-wry-2.12.2` | `tauri-runtime-wry` 2.12.2 | 5 files / 202 lines | #13020 | #13020 (refresh); carried from 2.11.2: `6c070616cc`, `9cf4a7ec0a`, `9e5afba10f`, #12652 | Bridges Tao + Wry into the Tauri runtime. `Cargo.toml` gtk4/webkit6 deps, `src/lib.rs` `gtk::glib::IsA` -> `gtk::prelude::IsA`, `src/monitor/linux.rs` `workarea()` -> `geometry()` fallback (GTK4 removed monitor workarea), `src/webview.rs` `webkit6::WebView` type alias, and the GTK4 `GestureClick`/`SurfaceEdge` module of `src/undecorated_resizing.rs`, whose gesture captures a *weak* webview ref (#12652). **#13020**: that module is spliced onto upstream 2.12.2, whose Windows half moved to edition-2024 `unsafe` blocks. Upstream 2.12's GTK3 resize-inset hover cursor and its `Propagation::Stop` on a resize press are not ported: GTK4 controller propagation and claim semantics differ and need interactive Linux validation, so the 2.11 GTK4 behavior is kept. |
| `tauri-2.12.4` | `tauri` 2.12.4 | 11 files / 131 lines | #13020 | #13020 (refresh); carried from 2.11.2: `b2155a581f`, `6c070616cc`, `9cf4a7ec0a`, `81d0a00a97` | Root feature aggregation. `Cargo.toml` `wry` feature list `webkit2gtk` -> `webkit6` and gtk4 deps. `muda` is requested with `["serde"]` only: upstream asks for `gtk3`, and with neither `gtk3` nor `gtk4` muda 0.20 compiles its own no-op backend, so Linux menu-to-GTK attachment stays a documented no-op (`menu/menu.rs`, `menu/submenu.rs`, `manager/menu.rs`, `window/mod.rs` gate the attachment call sites to macOS/Windows). **#13020**: `tray-icon` is requested with `["serde"]` only, because upstream's `libappindicator` feature re-enables GTK3 through `muda-gtk3`. Doc-comment links (`webkit6`) and a whitespace-only `mobile/ios-api/Sources/Tauri/Logger.swift` reformat are unchanged from the 2.11.2 delta. |
| `tray-icon-0.25.1` | `tray-icon` 0.25.1 | 8 files / 404 lines | #13020 | #13020 (refresh); carried from 0.23.1: `833d4e02d3`, #9939 | Tray icon backend for Tauri. 0.25 splits Linux into a GTK3 AppIndicator backend (`libappindicator`, on by default, also enables `muda-gtk3`) and a pure-Rust D-Bus backend (`ksni`), and refuses to compile with neither. The patch removes the `libappindicator`/`muda-gtk3` features, their `dirs`/`libappindicator`/`gtk` dependencies and the GTK backend source, so GTK3 cannot be switched back on. Without `ksni`, Linux/BSD use `src/platform_impl/disabled.rs`, whose constructor returns an unsupported-platform error, so Linux tray behavior is unchanged; Windows/macOS support is intact. The crate docs and `README.md` carry "Maekon E31 patch note" comments. **#9939** (stable Windows tray UIA identity: `NIF_SHOWTIP`, `NIM_SETVERSION`/`NOTIFYICON_VERSION_4`, `NIN_SELECT`/`NIN_KEYSELECT`, LOWORD notification decoding, registration diagnostics) changed the vendored 0.23.1 Windows backend without regenerating its delta; #13020 carries it onto 0.25.1's GUID/`NIS_HIDDEN` visibility code and records it here. |
| `tauri-runtime-2.12.2` | `tauri-runtime` 2.12.2 | 3 files / 18 lines | #13020 | #13020 (refresh); carried from 2.11.2: `6c070616cc`, `9cf4a7ec0a` | Smallest real delta. `Cargo.toml` gtk4/webkit6 deps, `src/webview.rs` and `src/window.rs` `webkit2gtk::WebView`/`gtk::glib::IsA` type/trait-path updates to match the GTK4-clean Wry/Tao types flowing through this crate's trait definitions. |
| `tauri-plugin-wdio-webdriver-1.5.0` | `tauri-plugin-wdio-webdriver` 1.5.0 | 4 files / 67 lines | #13020 | #13020 (refresh); carried from 1.2.0: `1655c4e8f3`, `d9855cc989`, `1037939bc5`, `0c79148599`, `fcff775229` | Active WebDriver cell. #11074 found that the registry crate still selected GTK3/`webkit2gtk` beside the patched Tauri runtime's GTK4/`webkit6` `WebView`. Upstream 1.2.0 -> 1.5.0 did not change `src/platform/linux.rs`, so the port carries over unchanged: the Linux manifest, WebKit prelude and JavaScriptCore crate paths, GDK4 texture snapshot encoding and print operation aligned with `cairo-rs` 0.21 / `glib` 0.22 / `gtk4` 0.11 / `javascriptcore6` 0.6 / `webkit6` 0.6, two immutable upstream README links for the link-clean public export, and three rustdoc trailing spaces normalized verbatim. The package-local `permissions/autogenerated/reference.md` (written into the crate by the plugin build script) is kept so a build does not dirty the tree; it was missing from the 1.2.0 delta. `tauri-plugin-wdio` is 1.5.0 too. |
| `tauri-plugin-webdriver-0.2.1` | `tauri-plugin-webdriver` 0.2.1 | 3 files / ~23 lines | `a94c1f3d84` (vendor + modification are the **same commit** — see "Known issues") | none | Historical predecessor, no longer referenced by `[patch.crates-io]` after #11074. It is retained only to preserve the earlier #4230/#7722 delta and must not be substituted for the active WDIO package: doing so would discard later upstream behavior. |

**Retired in #13020:** `muda-0.19.1`. Its delta replaced the GTK3 menu backend with an in-process no-op
fallback on Linux. muda 0.20 ships the same thing upstream: with neither `gtk3` nor `gtk4` enabled it
compiles a `noop` backend. muda now resolves from crates.io and the patched `tauri` requests it with
`["serde"]` only. If a future `tauri` or `tray-icon` refresh re-enables `muda/gtk3`, the GTK3 line
reappears in `Cargo.lock` and the audit's GLib row blocks it.

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

### Vendored MSRV vs. workspace MSRV

The workspace declares `rust-version = "1.95.0"` (`clients/maekon-client/Cargo.toml`
`[workspace.package]`). After #13020 every active vendored crate declares a lower or equal floor
(`tao`/`wry` 1.85, `tauri`/`tauri-runtime`/`tauri-runtime-wry`/`tray-icon` 1.90,
`tauri-plugin-wdio-webdriver` 1.77). The retired `tauri-plugin-webdriver-0.2.1` declares 1.90, which
used to exceed the old 1.88.0 workspace floor; it no longer does and stays absent from the resolved
graph. Do not edit that provenance-only manifest.

### Commit boundaries and delta drift

For `tray-icon-0.23.1` and `tauri-plugin-webdriver-0.2.1`, the initial
vendor (unpacking the pristine crates.io source into `patches/`) and the
first source modification landed in the same commit (`b2155a581f` and
`a94c1f3d84` respectively). For the other 6 original crates, the vendor commit
(`a94c1f3d84`) is a clean, unmodified copy of the pristine crate, and every
GTK4 source change is a separate later `fix:` commit. The WDIO 1.2.0 package
(replaced by 1.5.0 in #13020) also had a clean `e97dc47c47` -> `1655c4e8f3`/`d9855cc989`/
`1037939bc5`/`0c79148599`/`fcff775229` boundary. This means git
history alone cannot reconstruct a pristine baseline for those 2 crates —
which is exactly the gap `UPSTREAM-DELTA.patch` now closes going forward:
regardless of commit granularity, the pristine baseline is always one
`curl`+`tar` away, not an archaeology exercise.

The #13020 refresh vendored each active crate in a pristine commit separate from its port commit,
but squash merges collapse PR history on `main`, so `UPSTREAM-DELTA.patch` stays the durable record.
It also found two deltas that had drifted from their trees: #9939 changed the vendored `tray-icon`
Windows backend and the WDIO package gained its generated permission reference without either delta
being regenerated. Regenerate a crate's delta in the same change as any edit to that crate, not only
during a refresh.

## Refresh procedure

Follow this whenever a CVE/security fix, or a routine version bump, needs
to land in one of these crates. Do this per-crate, not as a batch — each
crate's modifications are independent and a batch refresh makes review and
rollback harder. When upstream versions are coupled (tauri 2.12 requires
tauri-runtime/-wry 2.12, which require tao 0.37 and wry 0.57, plus muda 0.20 and
tray-icon 0.25), refresh them in one PR but keep one port commit per crate (#13020).

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

   Before merging, check that the vendored files still use upstream's formatting; a reformat turns the
   whole file into one conflict. Upstream `tao` is reproduced exactly by
   `rustfmt --edition 2021 --config tab_spaces=2`; `tray-icon` uses rustfmt defaults. Format the vendored
   side to match upstream first, then merge (#13020 cut the `tao` delta from ~5378 to 1840 lines this way).

   Resolve any conflicts by hand, consulting the "why" column above for the
   intent of each hunk (most hunks are GTK3->GTK4/WebKitGTK6 API renames —
   if upstream itself ships GTK4 support by the time you do this, most or
   all hunks should simply stop applying because the upstream code already
   matches the target state; see "Exit criteria" below).

3. **Commit pristine-vendor and re-applied modifications as separate
   commits.** This preserves the boundary that `tao`, `wry`, `tauri`,
   `tauri-runtime`, and `tauri-runtime-wry` already have and that
   `tray-icon` and the historical `tauri-plugin-webdriver` predecessor are missing:
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

   The crate-specific rows find the current `patches/<crate>-<version>/` directory themselves and block
   when their target is missing (#13020: they used to name the old version and pass with 0 matches).

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

   Measured after #13020 (`cargo tree --target x86_64-pc-windows-msvc -i ...`): `windows` still has 2
   versions, but the vendored family (`tao`, `wry`, `tauri`, `tauri-runtime`, `tauri-runtime-wry`) moved
   to `0.62.2` with `cpal`, `maekon-vision`, `sysinfo`, `xcap`, `webview2-com` and
   `tauri-winrt-notification`; `0.61.3` is left only for `enigo` 0.6.1. `windows-sys` still has 5
   versions: `0.52.0` (`self-replace`), `0.59.0` (`global-hotkey`), `0.60.2` (`keyring`, `rfd`),
   `0.61.2` (most crates, including `muda`/`tray-icon`) and `0.45.0` (no direct Windows-target
   dependent). None of the remaining splits are inside this fork.

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
`tauri-plugin-wdio-webdriver` patch (`maekon-app --features webdriver`) alongside
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

## GTK4 exit criteria — when to drop the Tauri-stack fork

Drop the vendored patches and go back to plain upstream crates.io
dependencies for every patched crate once upstream `tauri-apps/tao`,
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
3. Once every entry is removed, `bash scripts/audit-e31-experimental-patches.sh`
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


## Archive security forks (#13038)

These two corrections are independent of the GTK4 migration above. Their
removal criteria are in their own provenance documents, not the GTK4 criteria.
Both immutable crate archives were checksum-verified and committed pristine in
`f57cfe282c`; the local security changes are separate in the #13038 change.

| Crate | Local correction | Provenance and removal criteria |
| --- | --- | --- |
| `zip` 9.0.5 | Preserve the upstream EOCD complexity fixes while bounding fallback allocation and reads; repair empty AES reads, abort/comment finalization, and use the equivalent libc platform permission call. Four upstream unit tests use a per-run password instead of a literal (#8685). | [ZIP source review and validation](zip-9.0.5/MAEKON-PATCH.md) |
| `ppmd-rust` 1.5.1 | Require the inner IO type's independent Send or Sync bound on all six codecs. Retain upstream heap and minimum-memory fixes. | [PPMd soundness fix and trait regressions](ppmd-rust-1.5.1/MAEKON-PATCH.md) |

Each directory contains the exact `UPSTREAM-DELTA.patch`; those deltas exclude
this local `MAEKON-PATCH.md` and the delta file itself in addition to the
packaging exclusions above. The workspace keeps ZIP's default codecs and adds
`unreserved` to preserve untouched XLSX UID/GID extra fields during raw copy.
`audit-as-crates-io = false` identifies these reviewed local forks; it does not
certify the defective published versions. Advisory checks, existing publisher
trust windows and all transitive audit requirements remain in force.

## OCR and time dependency overrides

| Crate | Local change | Source and removal criteria |
| --- | --- | --- |
| rstar 0.13.0 | Use the allocation-backed priority queue in the 0.13.0 nearest-neighbor implementation to preserve rc.11 OCR selections without heapless 0.8. | [rstar patch](rstar-0.13.0/MAEKON-PATCH.md) |
| tauri-utils 2.10.2 | Select the serde_with macros used by Tauri configuration without resolving the original chrono package. | [tauri-utils patch](tauri-utils-2.10.2/MAEKON-PATCH.md) |
