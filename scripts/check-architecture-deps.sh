#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

echo "[arch-deps] Verifying workspace runtime dependency direction"

# E2BIG guard (#8685 R01): cargo metadata JSON is far larger than the kernel
# argv+env limit on hosted runners — pass it via a temp file, never via an
# environment variable ("Argument list too long", exit 126 on public CI).
METADATA_FILE="$(mktemp)"
trap 'rm -f "$METADATA_FILE"' EXIT
cargo metadata --format-version 1 --no-deps > "$METADATA_FILE"

if command -v node >/dev/null 2>&1; then
  ARCH_DEPS_METADATA_FILE="$METADATA_FILE" node <<'JS'
const fs = require('node:fs');
const metadata = JSON.parse(fs.readFileSync(process.env.ARCH_DEPS_METADATA_FILE, 'utf8'));
const packages = metadata.packages;
const workspaceNames = new Set(packages.map((pkg) => pkg.name));

// Normal/runtime dependency allowlist.
// Dev/build-only edges are intentionally ignored so architecture guardrails
// focus on production crate coupling.
const allowedRuntimeEdges = new Map([
  ["maekon-core", []],
  ["maekon-api-contracts", ["maekon-core"]],
  ["maekon-lint", []],
  ["maekon-audio", ["maekon-core"]],
  ["maekon-monitor", ["maekon-core"]],
  ["maekon-vision", ["maekon-core"]],
  ["maekon-network", ["maekon-core", "maekon-api-contracts"]],
  ["maekon-storage", ["maekon-core"]],
  ["maekon-suggestion", ["maekon-core"]],
  ["maekon-web", ["maekon-core", "maekon-api-contracts"]],
  ["maekon-automation", ["maekon-core"]],
  ["maekon-analysis", ["maekon-core"]],
  ["maekon-embedding", ["maekon-core"]],
  ["maekon-sandbox-worker", ["maekon-core"]],
  [
    "maekon-app",
    [
      "maekon-core",
      "maekon-api-contracts",
      "maekon-audio",
      "maekon-monitor",
      "maekon-vision",
      "maekon-network",
      "maekon-storage",
      "maekon-suggestion",
      "maekon-web",
      "maekon-automation",
      "maekon-analysis",
      "maekon-embedding",
    ],
  ],
]);

const errors = [];
const lines = [];

for (const pkg of [...packages].sort((a, b) => a.name.localeCompare(b.name))) {
  const name = pkg.name;
  if (!allowedRuntimeEdges.has(name)) {
    errors.push(`missing allowlist entry for workspace package: ${name}`);
    continue;
  }

  const runtimeDeps = [...new Set(
    pkg.dependencies
      .filter((dep) => dep.path !== null && dep.kind === null && workspaceNames.has(dep.name))
      .map((dep) => dep.name),
  )].sort();
  lines.push(`${name}: ${runtimeDeps.length > 0 ? runtimeDeps.join(", ") : "(none)"}`);

  const allowed = new Set(allowedRuntimeEdges.get(name));
  const unexpected = runtimeDeps.filter((dep) => !allowed.has(dep));
  if (unexpected.length > 0) {
    errors.push(`${name} has unexpected runtime workspace deps: ${unexpected.join(", ")}`);
  }
}

const unknownAllowlistEntries = [...allowedRuntimeEdges.keys()]
  .filter((name) => !workspaceNames.has(name))
  .sort();
if (unknownAllowlistEntries.length > 0) {
  errors.push(
    `allowlist references packages that are no longer in the workspace: ${unknownAllowlistEntries.join(", ")}`,
  );
}

if (errors.length > 0) {
  console.error("[arch-deps] Runtime dependency direction check failed:");
  for (const error of errors) {
    console.error(`  - ${error}`);
  }
  console.error("[arch-deps] Current runtime dependency snapshot:");
  for (const line of lines) {
    console.error(`  - ${line}`);
  }
  process.exit(1);
}

console.log("[arch-deps] Runtime dependency snapshot:");
for (const line of lines) {
  console.log(`  - ${line}`);
}
console.log("[arch-deps] Dependency direction check passed");
JS
  exit $?
fi

ARCH_DEPS_METADATA_FILE="$METADATA_FILE" python3 - <<'PY'
import json
import os
import sys

with open(os.environ["ARCH_DEPS_METADATA_FILE"], "r", encoding="utf-8") as _f:
    metadata = json.load(_f)
packages = metadata["packages"]
workspace_names = {pkg["name"] for pkg in packages}

# Normal/runtime dependency allowlist.
# Dev/build-only edges are intentionally ignored so architecture guardrails
# focus on production crate coupling.
allowed_runtime_edges = {
    "maekon-core": set(),
    "maekon-api-contracts": {"maekon-core"},
    "maekon-lint": set(),
    "maekon-audio": {"maekon-core"},
    "maekon-monitor": {"maekon-core"},
    "maekon-vision": {"maekon-core"},
    "maekon-network": {"maekon-core", "maekon-api-contracts"},
    "maekon-storage": {"maekon-core"},
    "maekon-suggestion": {"maekon-core"},
    "maekon-web": {"maekon-core", "maekon-api-contracts"},
    "maekon-automation": {"maekon-core"},
    "maekon-analysis": {"maekon-core"},
    "maekon-embedding": {"maekon-core"},
    "maekon-sandbox-worker": {"maekon-core"},
    "maekon-app": {
        "maekon-core",
        "maekon-api-contracts",
        "maekon-audio",
        "maekon-monitor",
        "maekon-vision",
        "maekon-network",
        "maekon-storage",
        "maekon-suggestion",
        "maekon-web",
        "maekon-automation",
        "maekon-analysis",
        "maekon-embedding",
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
