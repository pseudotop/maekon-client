#!/usr/bin/env bash
set -euo pipefail

MANIFEST="src-tauri/Cargo.toml"

python3 - << 'PY'
import json
from pathlib import Path, PurePosixPath
import tomllib

manifest = Path("src-tauri/Cargo.toml")
if not manifest.exists():
    raise SystemExit(f"missing manifest: {manifest}")

content = manifest.read_text(encoding="utf-8")
data = tomllib.loads(content)

license_file = (
    data.get("package", {})
    .get("metadata", {})
    .get("deb", {})
    .get("license-file")
)

if not isinstance(license_file, list) or len(license_file) < 2:
    raise SystemExit("[package.metadata.deb].license-file must be a 2-element array")

license_path_raw = license_file[0]
if not isinstance(license_path_raw, str) or not license_path_raw.strip():
    raise SystemExit("deb license-file path is empty")

resolved = (manifest.parent / license_path_raw).resolve()
if not resolved.exists():
    raise SystemExit(
        f"deb license-file path does not exist: {license_path_raw} (resolved: {resolved})"
    )

print(f"deb metadata ok: license file -> {resolved}")

deb = data.get("package", {}).get("metadata", {}).get("deb", {})

# #11023: every source path the packaging references must exist. cargo-deb fails
# late and obscurely on a missing asset, and the AppArmor profile is the kind of
# file that gets moved during a tidy-up — at which point the package silently
# stops shipping the thing that keeps it startable on Ubuntu 24.04.
#
# `target/` paths are build outputs and are deliberately not required here.
for entry in deb.get("assets", []):
    if not isinstance(entry, list) or not entry:
        raise SystemExit(f"malformed deb asset entry: {entry!r}")
    src = entry[0]
    if not isinstance(src, str) or not src.strip():
        raise SystemExit(f"deb asset source is empty: {entry!r}")
    if src.startswith("target/"):
        continue
    asset_path = (manifest.parent / src).resolve()
    if not asset_path.exists():
        raise SystemExit(f"deb asset source does not exist: {src} (resolved: {asset_path})")
    print(f"deb metadata ok: asset -> {src}")

scripts_dir_raw = deb.get("maintainer-scripts")
if scripts_dir_raw is not None:
    scripts_dir = (manifest.parent / scripts_dir_raw).resolve()
    if not scripts_dir.is_dir():
        raise SystemExit(
            f"deb maintainer-scripts is not a directory: {scripts_dir_raw} (resolved: {scripts_dir})"
        )
    # A maintainer script dpkg cannot execute is the same as no script at all,
    # and dpkg reports it as a package error rather than a permissions one.
    for script in sorted(scripts_dir.iterdir()):
        if script.is_file() and not script.stat().st_mode & 0o111:
            raise SystemExit(f"deb maintainer script is not executable: {script}")
    print(f"deb metadata ok: maintainer-scripts -> {scripts_dir_raw}")

# A conffile that ships no corresponding asset would be declared and absent.
asset_targets = set()
for entry in deb.get("assets", []):
    dest = entry[1] if len(entry) > 1 else ""
    src_name = Path(entry[0]).name
    asset_targets.add("/" + str(Path(dest.strip("/")) / src_name))
for conf in deb.get("conf-files", []):
    if conf not in asset_targets:
        raise SystemExit(
            f"conf-files declares {conf} but no asset installs it; shipped assets: {sorted(asset_targets)}"
        )
    print(f"deb metadata ok: conffile -> {conf}")

# #13986: Tauri installs every `bundle.externalBin` beside the app executable in
# the installers it builds (MSI, NSIS, the .app), and the runtime depends on it:
# the sandbox worker is resolved ONLY in the directory of the running executable
# (crates/maekon-automation/src/sandbox/ipc.rs). cargo-deb never reads
# tauri.conf.json, so the .deb has to state the same placement by hand. rc.11
# left it out: the package installed, served its dashboard and passed every
# install check while no sandboxed action could start.
def installed_path(source: str):
    """Where cargo-deb installs `source`, as (path, mode); None when it is not an asset."""
    matches = [
        entry for entry in deb.get("assets", [])
        if isinstance(entry, list) and entry and entry[0] == source
    ]
    if not matches:
        return None
    if len(matches) > 1:
        raise SystemExit(f"deb assets declare {source} {len(matches)} times; declare it once")
    entry = matches[0]
    dest = entry[1] if len(entry) > 1 and isinstance(entry[1], str) else ""
    mode = entry[2] if len(entry) > 2 and isinstance(entry[2], str) else ""
    path = PurePosixPath("/" + dest.strip("/"))
    # cargo-deb: a destination ending in "/" is a directory and keeps the file name.
    if dest.endswith("/"):
        path = path / PurePosixPath(source).name
    return path, mode


app_bins = [
    target.get("name") for target in data.get("bin", [])
    if isinstance(target, dict) and isinstance(target.get("name"), str)
]
if len(app_bins) != 1:
    raise SystemExit(f"expected exactly one [[bin]] in {manifest}, found {app_bins}")
app = installed_path(f"target/release/{app_bins[0]}")
if app is None:
    raise SystemExit(
        f"deb assets do not install target/release/{app_bins[0]}; "
        "the externalBin placement below is checked against it"
    )

tauri_conf = json.loads((manifest.parent / "tauri.conf.json").read_text(encoding="utf-8"))
external_bins = tauri_conf.get("bundle", {}).get("externalBin", [])
if not isinstance(external_bins, list) or not all(isinstance(b, str) and b for b in external_bins):
    raise SystemExit(f"tauri.conf.json bundle.externalBin is malformed: {external_bins!r}")
for external in external_bins:
    name = PurePosixPath(external).name
    expected = app[0].parent / name
    worker = installed_path(f"target/release/{name}")
    if worker is None:
        raise SystemExit(
            f"the .deb does not install externalBin '{name}' beside the app: add "
            f'["target/release/{name}", "{str(app[0].parent).lstrip("/")}/", "755"] to '
            "[package.metadata.deb].assets (#13986) — the runtime looks for it only "
            f"next to {app[0]}"
        )
    if worker[0] != expected:
        raise SystemExit(
            f"the .deb installs externalBin '{name}' at {worker[0]}, but the runtime "
            f"looks for it only at {expected}, beside {app[0]} (#13986)"
        )
    try:
        mode = int(worker[1], 8)
    except ValueError:
        mode = 0
    if mode & 0o111 != 0o111:
        raise SystemExit(
            f"the .deb installs externalBin '{name}' with mode {worker[1]!r}; the app "
            "spawns it as the signed-in user, so it must be executable by everyone (755)"
        )
    print(f"deb metadata ok: externalBin {name} -> {worker[0]} (beside {app[0]})")

# #11023: releases are built in the PUBLIC repo from an explicit export
# allowlist. `export-public-repo.sh` already refuses to export a workspace member
# that is missing from that list, but nothing covered packaging inputs — so a deb
# asset left off the list would export cleanly and the published .deb would
# quietly ship without it. That is how the AppArmor profile could be present
# here, absent for users, and green everywhere in between.
include_file = manifest.parent.parent / "scripts" / "public-repo-include.txt"
if not include_file.exists():
    print(
        f"deb metadata: export coverage NOT checked — {include_file} is absent "
        "(expected when running inside the exported public repo)"
    )
else:
    entries = [
        line.strip()
        for line in include_file.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]

    def covered(rel_path: str) -> bool:
        return any(rel_path == e or rel_path.startswith(e.rstrip("/") + "/") for e in entries)

    packaging_inputs = [
        entry[0] for entry in deb.get("assets", [])
        if isinstance(entry, list) and entry and isinstance(entry[0], str)
        and not entry[0].startswith("target/")
    ]
    if scripts_dir_raw is not None:
        packaging_inputs.append(str(scripts_dir_raw))

    for rel in packaging_inputs:
        exported_path = f"{manifest.parent.name}/{rel}"
        if not covered(exported_path):
            raise SystemExit(
                f"packaging input {exported_path} is not covered by "
                "scripts/public-repo-include.txt — it would be missing from the public "
                "export, and the published package would ship without it"
            )
        print(f"deb metadata ok: exported -> {exported_path}")
PY
