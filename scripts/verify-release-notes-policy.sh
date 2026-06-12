#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

CHANGELOG_PATH="${REPO_ROOT}/CHANGELOG.md"
VERSION=""
PUBLIC_MODE=0
PRINT_SECTION=0
MIN_BULLETS="${MAEKON_RELEASE_NOTES_MIN_BULLETS:-1}"
MIN_CHARS="${MAEKON_RELEASE_NOTES_MIN_CHARS:-80}"

usage() {
  cat <<'EOF'
Usage: scripts/verify-release-notes-policy.sh --version <version> [options]

Options:
  --version <version>      Release version without the v prefix.
  --changelog <path>       CHANGELOG.md path. Defaults to repository CHANGELOG.md.
  --public                 Enforce public-release policy. Kept explicit at call sites.
  --print-section          Print the validated section body instead of a pass summary.
  --min-bullets <count>    Minimum bullets under Keep a Changelog categories. Default: 1.
  --min-chars <count>      Minimum section body characters. Default: 80.
EOF
}

resolve_python() {
  local candidate
  if [[ -n "${PYTHON:-}" ]] && "$PYTHON" -c 'import sys' >/dev/null 2>&1; then
    printf '%s\n' "$PYTHON"
    return 0
  fi

  for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1 \
      && "$candidate" -c 'import sys' >/dev/null 2>&1; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  if [[ -n "${USERPROFILE:-}" ]] && command -v cygpath >/dev/null 2>&1; then
    candidate="$(cygpath -u "$USERPROFILE")/.cache/codex-runtimes/codex-primary-runtime/dependencies/python/python.exe"
    if [[ -x "$candidate" ]] && "$candidate" -c 'import sys' >/dev/null 2>&1; then
      printf '%s\n' "$candidate"
      return 0
    fi
  fi

  return 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --changelog)
      CHANGELOG_PATH="${2:-}"
      shift 2
      ;;
    --public)
      PUBLIC_MODE=1
      shift
      ;;
    --print-section)
      PRINT_SECTION=1
      shift
      ;;
    --min-bullets)
      MIN_BULLETS="${2:-}"
      shift 2
      ;;
    --min-chars)
      MIN_CHARS="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "${VERSION}" ]]; then
  echo "--version is required" >&2
  usage >&2
  exit 2
fi

if [[ "${PUBLIC_MODE}" -ne 1 ]]; then
  echo "--public is required so strict release-note enforcement is explicit" >&2
  exit 2
fi

if ! [[ "${MIN_BULLETS}" =~ ^[0-9]+$ ]] || ! [[ "${MIN_CHARS}" =~ ^[0-9]+$ ]]; then
  echo "--min-bullets and --min-chars must be non-negative integers" >&2
  exit 2
fi

if ! PYTHON_BIN="$(resolve_python)"; then
  echo "Python is required to validate release notes policy" >&2
  exit 1
fi

"$PYTHON_BIN" - "${CHANGELOG_PATH}" "${VERSION}" "${MIN_BULLETS}" "${MIN_CHARS}" "${PRINT_SECTION}" <<'PY'
import re
import sys
from pathlib import Path

path = Path(sys.argv[1])
version = sys.argv[2]
min_bullets = int(sys.argv[3])
min_chars = int(sys.argv[4])
print_section = sys.argv[5] == "1"

allowed_categories = {
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
}

placeholder_patterns = [
    r"\bTODO\b",
    r"\bTBD\b",
    r"\bWIP\b",
    r"\bplaceholder\b",
    r"\bcoming soon\b",
    r"\brelease notes?\s+pending\b",
    r"\bto be written\b",
    r"\bsee\s+(?:\[)?CHANGELOG\.md",
    r"\bgit[- ]cliff\b",
]


def fail(message: str) -> None:
    print(f"release notes policy failure: {message}", file=sys.stderr)
    raise SystemExit(1)


if not path.exists():
    fail(f"{path} does not exist")

lines = path.read_text(encoding="utf-8").splitlines()
header = re.compile(rf"^## \[{re.escape(version)}\] - (\d{{4}}-\d{{2}}-\d{{2}})$")
loose_header = re.compile(rf"^## \[{re.escape(version)}\](?:\s.*)?$")

strict_matches = [(idx, line) for idx, line in enumerate(lines) if header.match(line)]
loose_matches = [(idx, line) for idx, line in enumerate(lines) if loose_header.match(line)]
malformed_matches = [
    (idx, line) for idx, line in loose_matches if not header.match(line)
]

if malformed_matches:
    line_no, line = malformed_matches[0]
    fail(
        f"CHANGELOG.md section [{version}] must use '## [{version}] - YYYY-MM-DD' "
        f"(line {line_no + 1}: {line})"
    )

if len(strict_matches) != 1:
    if strict_matches:
        fail(
            f"CHANGELOG.md must contain exactly one dated section for [{version}], "
            f"found {len(strict_matches)}"
        )
    fail(f"CHANGELOG.md missing section '## [{version}] - YYYY-MM-DD'")

start = strict_matches[0][0]
end = len(lines)
for idx in range(start + 1, len(lines)):
    if lines[idx].startswith("## ["):
        end = idx
        break

body_lines = lines[start + 1 : end]
body = "\n".join(line.rstrip() for line in body_lines).strip()
compact_body = re.sub(r"\s+", " ", body)

if not body:
    fail(f"CHANGELOG.md section [{version}] is empty")

if len(compact_body) < min_chars:
    fail(
        f"CHANGELOG.md section [{version}] is too short "
        f"({len(compact_body)} chars, minimum {min_chars})"
    )

for pattern in placeholder_patterns:
    match = re.search(pattern, body, flags=re.IGNORECASE)
    if match:
        fail(
            f"CHANGELOG.md section [{version}] contains placeholder/fallback text: "
            f"{match.group(0)!r}"
        )

categories: list[str] = []
category_bullets = 0
current_category = None

for raw_line in body_lines:
    heading = re.match(r"^###\s+(.+?)\s*$", raw_line)
    if heading:
        current_category = heading.group(1)
        if current_category in allowed_categories:
            categories.append(current_category)
        continue

    if current_category in allowed_categories and re.match(r"^\s*-\s+\S", raw_line):
        category_bullets += 1

if not categories:
    fail(
        "CHANGELOG.md section "
        f"[{version}] must include at least one Keep a Changelog category: "
        + ", ".join(sorted(allowed_categories))
    )

if category_bullets < min_bullets:
    fail(
        f"CHANGELOG.md section [{version}] has {category_bullets} release-note bullets "
        f"under recognized categories, minimum {min_bullets}"
    )

if print_section:
    print(body)
else:
    print(
        "release notes policy passed: "
        f"version={version} categories={','.join(categories)} "
        f"bullets={category_bullets} chars={len(compact_body)}"
    )
PY
