#!/usr/bin/env bash
set -euo pipefail

MANIFEST="src-tauri/Cargo.toml"

python3 - << 'PY'
from pathlib import Path
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
