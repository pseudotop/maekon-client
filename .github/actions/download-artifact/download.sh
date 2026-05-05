#!/usr/bin/env bash

set -euo pipefail

info() {
  printf '[ARTIFACT-DL] %s\n' "$*"
}

fatal() {
  printf '[ARTIFACT-DL][ERROR] %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage:
  download.sh --path <dir> [--name <artifact>] [--pattern <glob>] [--merge-multiple]
              [--repo <owner/name>] [--run-id <id>]
EOF
}

REPOSITORY="${GITHUB_REPOSITORY:-}"
RUN_ID="${GITHUB_RUN_ID:-}"
DEST_PATH=""
ARTIFACT_NAME=""
ARTIFACT_PATTERN=""
MERGE_MULTIPLE_RAW="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)
      [[ $# -ge 2 ]] || fatal "--repo requires a value"
      REPOSITORY="$2"
      shift 2
      ;;
    --run-id)
      [[ $# -ge 2 ]] || fatal "--run-id requires a value"
      RUN_ID="$2"
      shift 2
      ;;
    --path)
      [[ $# -ge 2 ]] || fatal "--path requires a value"
      DEST_PATH="$2"
      shift 2
      ;;
    --name)
      [[ $# -ge 2 ]] || fatal "--name requires a value"
      ARTIFACT_NAME="$2"
      shift 2
      ;;
    --pattern)
      [[ $# -ge 2 ]] || fatal "--pattern requires a value"
      ARTIFACT_PATTERN="$2"
      shift 2
      ;;
    --merge-multiple)
      MERGE_MULTIPLE_RAW="true"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fatal "Unknown option: $1"
      ;;
  esac
done

[[ -n "$GH_TOKEN" ]] || fatal "GH_TOKEN is required"
[[ -n "$REPOSITORY" ]] || fatal "Repository is required"
[[ -n "$RUN_ID" ]] || fatal "Run ID is required"
[[ -n "$DEST_PATH" ]] || fatal "Destination path is required"

if [[ -n "$ARTIFACT_NAME" && -n "$ARTIFACT_PATTERN" ]]; then
  fatal "Use either --name or --pattern, not both"
fi
if [[ -z "$ARTIFACT_NAME" && -z "$ARTIFACT_PATTERN" ]]; then
  fatal "Either --name or --pattern is required"
fi

MERGE_MULTIPLE=0
case "$MERGE_MULTIPLE_RAW" in
  1|true|TRUE|True|yes|YES|Yes)
    MERGE_MULTIPLE=1
    ;;
esac

TMP_ROOT="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/artifact-download.XXXXXX")"
ARTIFACTS_FILE="$TMP_ROOT/artifacts.txt"
MATCHES_FILE="$TMP_ROOT/matches.txt"

cleanup() {
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

info "Listing artifacts for run $RUN_ID in $REPOSITORY"
gh api "repos/$REPOSITORY/actions/runs/$RUN_ID/artifacts?per_page=100" \
  --paginate \
  --jq '.artifacts[] | select(.expired == false) | .name' > "$ARTIFACTS_FILE"

while IFS= read -r artifact_name; do
  [[ -n "$artifact_name" ]] || continue

  if [[ -n "$ARTIFACT_NAME" ]]; then
    [[ "$artifact_name" == "$ARTIFACT_NAME" ]] || continue
  else
    case "$artifact_name" in
      $ARTIFACT_PATTERN) ;;
      *) continue ;;
    esac
  fi

  printf '%s\n' "$artifact_name" >> "$MATCHES_FILE"
done < "$ARTIFACTS_FILE"

if [[ ! -s "$MATCHES_FILE" ]]; then
  if [[ -n "$ARTIFACT_NAME" ]]; then
    fatal "Artifact '$ARTIFACT_NAME' not found in run $RUN_ID"
  fi
  fatal "No artifacts matching '$ARTIFACT_PATTERN' found in run $RUN_ID"
fi

mkdir -p "$DEST_PATH"
MATCH_COUNT="$(grep -c . "$MATCHES_FILE" || true)"
info "Downloading $MATCH_COUNT artifact(s) into $DEST_PATH"

download_single() {
  local artifact_name="$1"
  local target_dir="$2"
  mkdir -p "$target_dir"
  gh run download "$RUN_ID" --repo "$REPOSITORY" -n "$artifact_name" -D "$target_dir"
}

if [[ "$MERGE_MULTIPLE" -eq 1 ]]; then
  index=0
  while IFS= read -r artifact_name; do
    index=$((index + 1))
    stage_dir="$TMP_ROOT/merge-$index"
    download_single "$artifact_name" "$stage_dir"
    cp -R "$stage_dir"/. "$DEST_PATH"/
  done < "$MATCHES_FILE"
elif [[ "$MATCH_COUNT" -eq 1 ]]; then
  ARTIFACT_TO_DOWNLOAD="$(cat "$MATCHES_FILE")"
  download_single "$ARTIFACT_TO_DOWNLOAD" "$DEST_PATH"
else
  while IFS= read -r artifact_name; do
    download_single "$artifact_name" "$DEST_PATH/$artifact_name"
  done < "$MATCHES_FILE"
fi

info "Artifact download completed"
