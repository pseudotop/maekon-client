#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  ./scripts/export-public-repo.sh [--dry-run] [--worktree] [--no-commit] <destination-dir> [source-ref]

Examples:
  ./scripts/export-public-repo.sh /tmp/maekon-client-public
  ./scripts/export-public-repo.sh /tmp/maekon-client-public codex/release-web-gates-qa-connected-hardening
  ./scripts/export-public-repo.sh --dry-run /tmp/maekon-client-public-smoke
  ./scripts/export-public-repo.sh --dry-run --worktree

Behavior:
  1. Exports a clean snapshot of <source-ref> (default: HEAD), or the current
     working tree when --worktree is passed.
  2. Applies exclusion rules from scripts/public-repo-exclude.txt.
  3. Validates the public-minimal export profile.
  4. Initializes a fresh Git history in <destination-dir> with one initial commit,
     unless --no-commit is passed.

This script does not push to any remote.
USAGE
}

DRY_RUN=0
INIT_GIT=1
DEST_CREATED=0
EXPORT_WORKTREE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --worktree)
      EXPORT_WORKTREE=1
      shift
      ;;
    --no-commit)
      INIT_GIT=0
      shift
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "error: unknown option: $1" >&2
      usage
      exit 1
      ;;
    *)
      break
      ;;
  esac
done

DEST_DIR="${1:-}"
SOURCE_REF="${2:-HEAD}"

if [[ -z "$DEST_DIR" && "$DRY_RUN" == "1" ]]; then
  DEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/maekon-client-public.XXXXXX")"
  DEST_CREATED=1
elif [[ -z "$DEST_DIR" ]]; then
  usage
  exit 1
fi

if [[ "$DEST_CREATED" == "0" && -e "$DEST_DIR" ]]; then
  echo "error: destination already exists: $DEST_DIR" >&2
  exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel)"
EXCLUDE_FILE="$REPO_ROOT/scripts/public-repo-exclude.txt"

apply_exclude_rules() {
  local rule
  shopt -s nullglob dotglob
  while IFS= read -r rule; do
    [[ -z "$rule" || "$rule" =~ ^# ]] && continue
    rule="${rule%/}"
    # Intentional word splitting is avoided; glob expansion is only applied to
    # repository-relative rules from the controlled exclude file.
    local matches=( "$DEST_DIR"/$rule )
    if (( ${#matches[@]} == 0 )); then
      continue
    fi
    rm -rf -- "${matches[@]}"
  done < "$EXCLUDE_FILE"
  shopt -u nullglob dotglob
}

validate_public_export() {
  local missing=0
  local required_paths=(
    "Cargo.toml"
    "README.md"
    "LICENSE"
    ".github/dependabot.yml"
    "assets/brand/cli-banner.txt"
    "specs/providers/provider-surface-catalog.json"
  )
  local forbidden_paths=(
    "CLAUDE.md"
    ".github/workflows/dependabot-auto-merge.yml"
    "artifacts"
    "docs/superpowers"
    "docs/reviews"
    "docs/research"
    "docs/roadmap"
    "docs/plan"
    "docs/specs"
    "docs/migration"
    "docs/qa/runs"
    "docs/guides/public-repo-launch-playbook.md"
    "docs/guides/public-repo-launch-playbook.ko.md"
    "docs/PHASE-HISTORY.md"
    "docs/STATUS.md"
    "docs/STATUS.ko.md"
    "crates/oneshim-web/frontend/docs"
    "crates/oneshim-web/frontend/storybook-review-static"
    "crates/oneshim-web/frontend/storybook-static"
    "crates/oneshim-web/frontend/playwright-report"
    "crates/oneshim-web/frontend/coverage"
    "tests/private"
    "server"
    "backoffice"
    "terraform"
    ".env"
    ".claude"
    ".worktrees"
    ".superpowers"
  )

  for path in "${required_paths[@]}"; do
    if [[ ! -e "$DEST_DIR/$path" ]]; then
      echo "error: public export is missing required path: $path" >&2
      missing=1
    fi
  done

  for path in "${forbidden_paths[@]}"; do
    if [[ -e "$DEST_DIR/$path" ]]; then
      echo "error: public export contains forbidden path: $path" >&2
      missing=1
    fi
  done

  if find "$DEST_DIR/specs" -mindepth 1 -type f ! -path "$DEST_DIR/specs/providers/*" -print -quit 2>/dev/null | grep -q .; then
    echo "error: public export contains non-provider root specs" >&2
    missing=1
  fi

  if find "$DEST_DIR" -name 'sbom.cdx.json' -print -quit 2>/dev/null | grep -q .; then
    echo "error: public export contains generated SBOM artifacts" >&2
    missing=1
  fi

  local scan_file
  scan_file="$(mktemp "${TMPDIR:-/tmp}/maekon-public-scan.XXXXXX")"
  local internal_volume_pattern="/Volumes""/ext"
  local generated_pattern="Generated with \[Claude Code\]"
  local ralph_pattern="ralph""-loop"
  local private_tests_pattern="tests/private""/client-rust"
  local legacy_email_pattern="[[:alnum:]._%+-]+@oneshim\\.dev"
  local high_confidence_pattern="(${internal_volume_pattern}|${generated_pattern}|${ralph_pattern}|${private_tests_pattern}|${legacy_email_pattern})"

  if grep -RInE --binary-files=without-match \
    --exclude-dir=.git \
    --exclude='*.lock' \
    "$high_confidence_pattern" \
    "$DEST_DIR" > "$scan_file"; then
    echo "error: public export contains high-confidence internal references:" >&2
    cat "$scan_file" >&2
    rm -f "$scan_file"
    missing=1
  else
    rm -f "$scan_file"
  fi

  scan_file="$(mktemp "${TMPDIR:-/tmp}/maekon-public-scan.XXXXXX")"
  local legacy_public_repo_pattern="pseudotop/oneshim""-client|raw.githubusercontent.com/pseudotop/oneshim""-client"
  if grep -RInE --binary-files=without-match \
    --exclude-dir=.git \
    --exclude='*.lock' \
    --exclude='CHANGELOG.md' \
    "$legacy_public_repo_pattern" \
    "$DEST_DIR" > "$scan_file"; then
    echo "error: public export contains stale public repository references:" >&2
    cat "$scan_file" >&2
    rm -f "$scan_file"
    missing=1
  else
    rm -f "$scan_file"
  fi

  if (( missing != 0 )); then
    exit 1
  fi
}

mkdir -p "$DEST_DIR"

echo "==> Exporting snapshot from ref: $SOURCE_REF"
if [[ "$EXPORT_WORKTREE" == "1" ]]; then
  echo "==> Using current working tree contents"
  rsync -a --delete \
    --exclude '.git/' \
    --exclude '.git' \
    --exclude 'target/' \
    --exclude '**/target/' \
    --exclude 'node_modules/' \
    --exclude '**/node_modules/' \
    --exclude 'dist/' \
    --exclude '**/dist/' \
    --exclude 'storybook-review-static/' \
    --exclude '**/storybook-review-static/' \
    --exclude 'storybook-static/' \
    --exclude '**/storybook-static/' \
    --exclude 'playwright-report/' \
    --exclude '**/playwright-report/' \
    --exclude 'coverage/' \
    --exclude '**/coverage/' \
    --exclude '.DS_Store' \
    "$REPO_ROOT/" "$DEST_DIR/"
else
  git -C "$REPO_ROOT" archive "$SOURCE_REF" | tar -xf - -C "$DEST_DIR"
fi

if [[ -f "$EXCLUDE_FILE" ]]; then
  echo "==> Applying exclude rules from: scripts/public-repo-exclude.txt"
  apply_exclude_rules
fi

echo "==> Validating public-minimal export"
validate_public_export

if [[ "$INIT_GIT" == "1" ]]; then
  echo "==> Initializing fresh Git history"
  git -C "$DEST_DIR" init -b main >/dev/null
  git -C "$DEST_DIR" add -A
  git -C "$DEST_DIR" \
    -c user.name="Maekon Public Export" \
    -c user.email="support@maekon.dev" \
    commit -m "chore: bootstrap public repository history" >/dev/null
else
  echo "==> Skipping Git initialization (--no-commit)"
fi

echo "==> Done"
echo "Public repo path: $DEST_DIR"
if [[ "$DRY_RUN" == "1" ]]; then
  echo "Dry-run export kept for inspection."
else
  echo "Next:"
  echo "  cd $DEST_DIR"
  if [[ "$INIT_GIT" == "1" ]]; then
    echo "  git log --oneline --decorate -n 1"
  fi
  echo "  git remote add origin <public-repo-url>"
  echo "  git push -u origin main"
fi
