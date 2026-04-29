#!/usr/bin/env bash
# Shared helpers for rc-first release automation.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKSPACE_CARGO_TOML="${REPO_ROOT}/Cargo.toml"
CARGO_LOCK_PATH="${REPO_ROOT}/Cargo.lock"
FRONTEND_PACKAGE_JSON="${REPO_ROOT}/crates/oneshim-web/frontend/package.json"
CHANGELOG_PATH="${REPO_ROOT}/CHANGELOG.md"

is_rc_version() {
  [[ "${1}" =~ ^[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$ ]]
}

is_stable_version() {
  [[ "${1}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

base_version() {
  printf '%s\n' "${1%%-rc.*}"
}

# Increment the patch component of a semver: 0.4.1 -> 0.4.2
next_patch_version() {
  local ver="${1}"
  local major minor patch
  IFS='.' read -r major minor patch <<< "${ver}"
  printf '%s.%s.%s\n' "${major}" "${minor}" "$((patch + 1))"
}

workspace_version() {
  python3 - "${WORKSPACE_CARGO_TOML}" <<'PY'
import re
import sys
from pathlib import Path

path = Path(sys.argv[1])
in_workspace_package = False

for raw_line in path.read_text(encoding="utf-8").splitlines():
    line = raw_line.strip()
    if line == "[workspace.package]":
        in_workspace_package = True
        continue
    if in_workspace_package and line.startswith("[") and line != "[workspace.package]":
        break
    if in_workspace_package:
        match = re.match(r'^version\s*=\s*"([^"]+)"$', line)
        if match:
            print(match.group(1))
            raise SystemExit(0)

raise SystemExit("Could not find [workspace.package] version field in Cargo.toml")
PY
}

frontend_version() {
  python3 - "${FRONTEND_PACKAGE_JSON}" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
print(json.loads(path.read_text(encoding="utf-8"))["version"])
PY
}

changelog_unreleased_count() {
  python3 - "${CHANGELOG_PATH}" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
count = sum(1 for line in path.read_text(encoding="utf-8").splitlines() if line == "## [Unreleased]")
print(count)
PY
}

require_single_unreleased_header() {
  local count
  count="$(changelog_unreleased_count)"
  if [[ "${count}" -ne 1 ]]; then
    echo "CHANGELOG.md must contain exactly one [Unreleased] header (found ${count})" >&2
    return 1
  fi
}

unreleased_section_content() {
  python3 - "${CHANGELOG_PATH}" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
lines = path.read_text(encoding="utf-8").splitlines()
found = False
body = []

for line in lines:
    if not found:
        if line == "## [Unreleased]":
            found = True
        continue
    if line.startswith("## ["):
        break
    body.append(line)

print("\n".join(line for line in body if line.strip()))
PY
}

populate_unreleased_section_from_generated_changelog() {
  GENERATED_CHANGELOG="${1}" python3 - "${CHANGELOG_PATH}" <<'PY'
import os
import sys
from pathlib import Path

path = Path(sys.argv[1])
generated = os.environ.get("GENERATED_CHANGELOG", "")


def extract_unreleased_body(lines: list[str]) -> list[str]:
    found = False
    body: list[str] = []
    for line in lines:
        if not found:
            if line == "## [Unreleased]":
                found = True
            continue
        if line.startswith("## ["):
            break
        body.append(line)
    while body and not body[0].strip():
        body.pop(0)
    while body and not body[-1].strip():
        body.pop()
    return body


current_lines = path.read_text(encoding="utf-8").splitlines()
count = sum(1 for line in current_lines if line == "## [Unreleased]")
if count != 1:
    raise SystemExit(
        f"CHANGELOG.md must contain exactly one [Unreleased] header (found {count})"
    )

new_body = extract_unreleased_body(generated.splitlines())
if not any(line.strip() for line in new_body):
    raise SystemExit("Generated changelog did not contain any [Unreleased] content")

updated: list[str] = []
i = 0
while i < len(current_lines):
    line = current_lines[i]
    if line == "## [Unreleased]":
        updated.append(line)
        updated.append("")
        updated.extend(new_body)
        updated.append("")
        i += 1
        while i < len(current_lines) and not current_lines[i].startswith("## ["):
            i += 1
        continue
    updated.append(line)
    i += 1

path.write_text("\n".join(updated).rstrip() + "\n", encoding="utf-8")
PY
}

workspace_lock_mismatches() {
  python3 - "${CARGO_LOCK_PATH}" "${1}" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
expected = sys.argv[2]
mismatches = []
content = path.read_text(encoding="utf-8")

try:
    import tomllib  # Python 3.11+

    for package in tomllib.loads(content).get("package", []):
        name = package.get("name")
        if not isinstance(name, str) or not name.startswith("oneshim-"):
            continue
        if "source" in package:
            continue
        version = package.get("version", "<missing>")
        if version != expected:
            mismatches.append((name, version))
except ModuleNotFoundError:
    name = None
    version = None
    has_source = False

    def flush() -> None:
        if not name or not name.startswith("oneshim-") or has_source:
            return
        if version != expected:
            mismatches.append((name, version or "<missing>"))

    for raw_line in content.splitlines():
        line = raw_line.strip()
        if line == "[[package]]":
            flush()
            name = None
            version = None
            has_source = False
            continue
        if line.startswith('name = "'):
            name = line.split('"', 2)[1]
            continue
        if line.startswith('version = "'):
            version = line.split('"', 2)[1]
            continue
        if line.startswith("source = "):
            has_source = True

    flush()

if mismatches:
    for name, version in mismatches:
        print(f"{name}\t{version}")
    raise SystemExit(1)
PY
}

sync_workspace_lockfile() {
  cargo metadata --format-version 1 --manifest-path "${WORKSPACE_CARGO_TOML}" >/dev/null
}

set_workspace_version() {
  python3 - "${WORKSPACE_CARGO_TOML}" "${1}" <<'PY'
import re
import sys
from pathlib import Path

path = Path(sys.argv[1])
version = sys.argv[2]
content = path.read_text(encoding="utf-8")

lines = content.splitlines()
out = []
in_workspace_package = False
replaced = False

for line in lines:
    if line.strip() == "[workspace.package]":
        in_workspace_package = True
        out.append(line)
        continue
    if in_workspace_package and line.startswith("[") and line.strip() != "[workspace.package]":
        in_workspace_package = False
    if in_workspace_package and re.match(r'^version\s*=\s*"[^"]+"$', line):
        out.append(f'version = "{version}"')
        replaced = True
    else:
        out.append(line)

if not replaced:
    raise SystemExit("Could not find [workspace.package] version field in Cargo.toml")

path.write_text("\n".join(out) + "\n", encoding="utf-8")
PY
}

set_frontend_version() {
  python3 - "${FRONTEND_PACKAGE_JSON}" "${1}" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
version = sys.argv[2]
data = json.loads(path.read_text(encoding="utf-8"))
data["version"] = version
path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
PY
}

changelog_has_entry() {
  grep -q "^## \[${1}\]" "${CHANGELOG_PATH}"
}

promote_unreleased_section() {
  python3 - "${CHANGELOG_PATH}" "${1}" "$(date +%Y-%m-%d)" <<'PY'
import re
import sys
from pathlib import Path

path = Path(sys.argv[1])
target_version = sys.argv[2]
target_date = sys.argv[3]
lines = path.read_text(encoding="utf-8").splitlines(keepends=True)

target_header = re.compile(rf"^## \[{re.escape(target_version)}\](?: - .*)?\n?$")
if any(target_header.match(line) for line in lines):
    raise SystemExit(f"CHANGELOG.md already has [{target_version}]")

unreleased_indices = [idx for idx, line in enumerate(lines) if line.startswith("## [Unreleased]")]
if not unreleased_indices:
    raise SystemExit("CHANGELOG.md is missing the [Unreleased] header")
if len(unreleased_indices) != 1:
    raise SystemExit(
        f"CHANGELOG.md must contain exactly one [Unreleased] header, found {len(unreleased_indices)}"
    )

unreleased_idx = unreleased_indices[0]

next_section_idx = len(lines)
for idx in range(unreleased_idx + 1, len(lines)):
    if lines[idx].startswith("## ["):
        next_section_idx = idx
        break

body = "".join(lines[unreleased_idx + 1:next_section_idx]).strip()
if not body:
    raise SystemExit("[Unreleased] section is empty")

insert_block = [f"## [{target_version}] - {target_date}\n"]
insert_block.extend(lines[unreleased_idx + 1:next_section_idx])
if insert_block[-1].strip():
    insert_block.append("\n")

new_lines = []
new_lines.extend(lines[: unreleased_idx + 1])
new_lines.append("\n")
new_lines.extend(insert_block)
new_lines.extend(lines[next_section_idx:])

path.write_text("".join(new_lines), encoding="utf-8")
PY
}

copy_changelog_section() {
  python3 - "${CHANGELOG_PATH}" "${1}" "${2}" "$(date +%Y-%m-%d)" <<'PY'
import re
import sys
from pathlib import Path

path = Path(sys.argv[1])
source_version = sys.argv[2]
target_version = sys.argv[3]
target_date = sys.argv[4]
lines = path.read_text(encoding="utf-8").splitlines(keepends=True)


def find_section(version: str):
    header = re.compile(rf"^## \[{re.escape(version)}\](?: - .*)?\n?$")
    start = None
    for idx, line in enumerate(lines):
        if header.match(line):
            start = idx
            break
    if start is None:
        return None
    end = len(lines)
    for idx in range(start + 1, len(lines)):
        if lines[idx].startswith("## ["):
            end = idx
            break
    return start, end


source = find_section(source_version)
if source is None:
    raise SystemExit(f"CHANGELOG.md missing source section [{source_version}]")

target = find_section(target_version)
source_body = "".join(lines[source[0] + 1:source[1]]).strip()

if target is not None:
    target_body = "".join(lines[target[0] + 1:target[1]]).strip()
    if target_body != source_body:
        raise SystemExit(
            f"CHANGELOG.md already has [{target_version}], but its body differs from [{source_version}]"
        )
    print(f"CHANGELOG.md already contains [{target_version}] with matching content")
    raise SystemExit(0)

insert_block = [f"## [{target_version}] - {target_date}\n"]
insert_block.extend(lines[source[0] + 1:source[1]])
if insert_block and insert_block[-1].strip():
    insert_block.append("\n")

lines[source[0]:source[0]] = insert_block
path.write_text("".join(lines), encoding="utf-8")
PY
}

changelog_section_body_matches() {
  python3 - "${CHANGELOG_PATH}" "${1}" "${2}" <<'PY'
import re
import sys
from pathlib import Path

path = Path(sys.argv[1])
left_version = sys.argv[2]
right_version = sys.argv[3]
lines = path.read_text(encoding="utf-8").splitlines()


def body(version: str):
    header = re.compile(rf"^## \[{re.escape(version)}\](?: - .*)?$")
    start = None
    for idx, line in enumerate(lines):
        if header.match(line):
            start = idx
            break
    if start is None:
        return None
    end = len(lines)
    for idx in range(start + 1, len(lines)):
        if lines[idx].startswith("## ["):
            end = idx
            break
    return "\n".join(lines[start + 1:end]).strip()


left = body(left_version)
right = body(right_version)
if left is None or right is None:
    raise SystemExit(1)
raise SystemExit(0 if left == right else 1)
PY
}

require_main_branch() {
  local current_branch
  current_branch="$(git rev-parse --abbrev-ref HEAD)"
  [[ "${current_branch}" == "main" ]]
}

require_clean_worktree() {
  git diff --quiet && git diff --cached --quiet
}

ensure_head_matches_tag() {
  local tag="$1"
  local tag_commit
  tag_commit="$(git rev-parse "${tag}^{commit}")"
  [[ "$(git rev-parse HEAD)" == "${tag_commit}" ]]
}

latest_rc_tag_for_base() {
  git tag -l "v${1}-rc.*" --sort=-version:refname | head -n1
}

allowed_promotion_file() {
  case "${1}" in
    Cargo.toml|Cargo.lock|CHANGELOG.md|crates/oneshim-web/frontend/package.json)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}
