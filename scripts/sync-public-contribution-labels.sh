#!/usr/bin/env bash
set -euo pipefail

repo="${1:-}"

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "required tool missing: $1" >&2
    exit 2
  fi
}

label_exists() {
  gh label list --repo "$repo" --limit 1000 --json name --jq '.[].name' \
    | grep -Fx -- "$1" >/dev/null 2>&1
}

sync_label() {
  local name="$1"
  local color="$2"
  local description="$3"

  if label_exists "$name"; then
    gh label edit "$name" --repo "$repo" --color "$color" --description "$description"
  else
    gh label create "$name" --repo "$repo" --color "$color" --description "$description"
  fi
}

require_tool gh
require_tool grep

if [ -z "$repo" ]; then
  repo="$(gh repo view --json nameWithOwner --jq '.nameWithOwner' 2>/dev/null || true)"
fi

if [ -z "$repo" ]; then
  echo "usage: $0 OWNER/REPO" >&2
  exit 2
fi

sync_label "lane:good-first-dx" "0E8A16" "Public-safe docs, setup, small tests, and beginner-friendly DX work"
sync_label "lane:local-feature" "1D76DB" "Local dashboard, settings, export, and UX work outside trust-core semantics"
sync_label "lane:provider-adapter" "5319E7" "Public provider metadata/spec and adapter compatibility work"
sync_label "lane:privacy-docs" "B60205" "Privacy, consent, PII, and safe evidence documentation"
sync_label "lane:trust-core" "D93F0B" "Consent, PII, capture, audio, automation, sandbox, updater, release, or local API security"
sync_label "lane:enterprise-contract" "6F42C1" "Managed sync, analytics, SSO/RBAC, compliance, admin, or enterprise contract work"
sync_label "lane:security-disclosure" "B60205" "Security disclosure handling; do not discuss vulnerability details publicly"

sync_label "risk:privacy" "B60205" "May affect consent, PII, capture, evidence, retention, or data minimization"
sync_label "risk:security" "D93F0B" "May affect sandboxing, local auth, update integrity, dependency trust, or secrets"
sync_label "risk:release" "FBCA04" "May affect packaging, signing, notarization, installers, updates, or public export"

sync_label "do-not-merge/security" "B60205" "Blocked until security/private handling is complete"
sync_label "do-not-merge/private-test" "B60205" "Blocked until required private gates are run and safely summarized"
sync_label "do-not-merge/needs-owner" "D93F0B" "Blocked until the relevant CODEOWNER or maintainer approves"
sync_label "do-not-merge/dco" "FBCA04" "Blocked until required Signed-off-by or legal attestation is present"

sync_label "ok-to-test" "0E8A16" "Maintainer has cleared the PR for maintainer-controlled test execution"
sync_label "security-reviewed" "0E8A16" "Security/privacy review has cleared the public handling path"
sync_label "imported-to-parent" "5319E7" "Public change has been imported into the parent source of truth"
