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
PY
