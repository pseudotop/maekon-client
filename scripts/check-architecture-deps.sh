#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

echo "[arch-deps] Verifying workspace runtime dependency direction"

METADATA_JSON="$(cargo metadata --format-version 1 --no-deps)"

ARCH_DEPS_METADATA="$METADATA_JSON" python3 - <<'PY'
import json
import os
import sys

metadata = json.loads(os.environ["ARCH_DEPS_METADATA"])
packages = metadata["packages"]
workspace_names = {pkg["name"] for pkg in packages}

# Normal/runtime dependency allowlist.
# Dev/build-only edges are intentionally ignored so architecture guardrails
# focus on production crate coupling.
allowed_runtime_edges = {
    "oneshim-core": set(),
    "oneshim-api-contracts": {"oneshim-core"},
    "oneshim-lint": set(),
    "oneshim-audio": {"oneshim-core"},
    "oneshim-monitor": {"oneshim-core"},
    "oneshim-vision": {"oneshim-core"},
    "oneshim-network": {"oneshim-core", "oneshim-api-contracts"},
    "oneshim-storage": {"oneshim-core"},
    "oneshim-suggestion": {"oneshim-core"},
    "oneshim-web": {"oneshim-core", "oneshim-api-contracts"},
    "oneshim-automation": {"oneshim-core"},
    "oneshim-analysis": {"oneshim-core"},
    "oneshim-embedding": {"oneshim-core"},
    "oneshim-sandbox-worker": {"oneshim-core"},
    "oneshim-app": {
        "oneshim-core",
        "oneshim-api-contracts",
        "oneshim-audio",
        "oneshim-monitor",
        "oneshim-vision",
        "oneshim-network",
        "oneshim-storage",
        "oneshim-suggestion",
        "oneshim-web",
        "oneshim-automation",
        "oneshim-analysis",
        "oneshim-embedding",
    },
}

errors: list[str] = []
lines: list[str] = []

for pkg in sorted(packages, key=lambda item: item["name"]):
    name = pkg["name"]
    if name not in allowed_runtime_edges:
        errors.append(f"missing allowlist entry for workspace package: {name}")
        continue

    runtime_deps = sorted(
        {
            dep["name"]
            for dep in pkg["dependencies"]
            if dep.get("path") is not None
            and dep.get("kind") is None
            and dep["name"] in workspace_names
        }
    )
    lines.append(f"{name}: {', '.join(runtime_deps) if runtime_deps else '(none)'}")

    unexpected = sorted(set(runtime_deps) - allowed_runtime_edges[name])
    if unexpected:
        errors.append(
            f"{name} has unexpected runtime workspace deps: {', '.join(unexpected)}"
        )

unknown_allowlist_entries = sorted(set(allowed_runtime_edges) - workspace_names)
if unknown_allowlist_entries:
    errors.append(
        "allowlist references packages that are no longer in the workspace: "
        + ", ".join(unknown_allowlist_entries)
    )

if errors:
    print("[arch-deps] Runtime dependency direction check failed:", file=sys.stderr)
    for error in errors:
        print(f"  - {error}", file=sys.stderr)
    print("[arch-deps] Current runtime dependency snapshot:", file=sys.stderr)
    for line in lines:
        print(f"  - {line}", file=sys.stderr)
    sys.exit(1)

print("[arch-deps] Runtime dependency snapshot:")
for line in lines:
    print(f"  - {line}")
print("[arch-deps] Dependency direction check passed")
PY
