#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 || $# -gt 5 ]]; then
  cat >&2 <<'USAGE'
Usage: notary-submit-and-poll.sh <artifact-path> <keychain-path> <profile-name> [timeout-secs] [poll-interval-secs]
USAGE
  exit 2
fi

artifact_path="$1"
keychain_path="$2"
profile_name="$3"
timeout_secs="${4:-3600}"
poll_interval_secs="${5:-30}"

if [[ ! -f "$artifact_path" ]]; then
  echo "Artifact not found: $artifact_path" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to parse notarytool JSON output." >&2
  exit 1
fi

artifact_name="$(basename "$artifact_path")"
log_dir="dist/notary-logs"
mkdir -p "$log_dir"

echo "::group::Submit for notarization ($artifact_name)"
submit_json="$(xcrun notarytool submit "$artifact_path" \
  --keychain "$keychain_path" \
  --keychain-profile "$profile_name" \
  --output-format json)"

echo "$submit_json" | jq '{id, name, createdDate}'

submission_id="$(echo "$submit_json" | jq -r '.id // empty')"
if [[ -z "$submission_id" ]]; then
  echo "Failed to parse submission id for $artifact_name" >&2
  exit 1
fi

echo "notary_submission_id=$submission_id"
echo "::endgroup::"

start_epoch="$(date +%s)"
attempt=1

while true; do
  info_json="$(xcrun notarytool info "$submission_id" \
    --keychain "$keychain_path" \
    --keychain-profile "$profile_name" \
    --output-format json)"

  status="$(echo "$info_json" | jq -r '.status // empty')"
  elapsed="$(( $(date +%s) - start_epoch ))"
  now_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

  echo "[${now_utc}] artifact=${artifact_name} submission_id=${submission_id} attempt=${attempt} elapsed_secs=${elapsed} status=${status}"
  echo "$info_json" | jq '{id, status, createdDate, name}'

  case "$status" in
    Accepted)
      break
      ;;
    Invalid|Rejected)
      echo "Notarization failed for $artifact_name with status=$status" >&2
      break
      ;;
    *)
      if (( elapsed >= timeout_secs )); then
        echo "Timed out waiting for notarization of $artifact_name after ${elapsed}s" >&2
        exit 124
      fi
      sleep "$poll_interval_secs"
      ;;
  esac

  attempt="$((attempt + 1))"
done

log_file="${log_dir}/${artifact_name}.notary-log.json"
xcrun notarytool log "$submission_id" \
  --keychain "$keychain_path" \
  --keychain-profile "$profile_name" \
  "$log_file" || true

if [[ -f "$log_file" ]]; then
  echo "::group::Notarization log summary ($artifact_name)"
  jq '{id, status, statusSummary, statusCode, issues}' "$log_file" || cat "$log_file"
  echo "::endgroup::"
fi

if [[ "$status" != "Accepted" ]]; then
  exit 1
fi

