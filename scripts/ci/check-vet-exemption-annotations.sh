#!/usr/bin/env bash
# check-vet-exemption-annotations.sh
#
# DEPRECATED compatibility warning gate.
#
# This script's 3-field schema (notes + reason + expiry) was never enforced in
# CI. The parent monorepo active gate lives in the repo-root workflow
# .github/workflows/cargo-supply-chain.yml and delegates to:
#
#   scripts/supply_chain/verify_exemption_expiry.py --fail-on-expired
#   scripts/supply_chain/gate_new_exemptions_review_by.py
#
# Both scripts enforce the `review-by:YYYY-MM-DD` annotation in the `notes`
# field — the only annotation format actually present in the existing exemptions (~635, varies).
# The stricter `reason` + `expiry` fields were never populated and applying
# this gate as-is would block all existing entries (~635, varies).
#
# OOS-TBD filed for proper 3-field rollout planning (F-SC-C34-01).
#
# The public snapshot may still call this script from the manual
# security-compliance.yml workflow as a non-blocking compatibility warning.
#
# Usage:
#   ./scripts/ci/check-vet-exemption-annotations.sh [supply-chain/config.toml]

echo "WARN: check-vet-exemption-annotations.sh is deprecated."
echo "      Parent active expiry enforcement uses verify_exemption_expiry.py in cargo-supply-chain.yml."
echo "      Public security-compliance.yml keeps this as a compatibility warning only."
echo "      See OOS-TBD F-SC-C34-01 for 3-field schema rollout plan."
exit 0
