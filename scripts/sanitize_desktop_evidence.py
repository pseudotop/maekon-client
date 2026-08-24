#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA_VERSION = "automation.gui.permission_evidence.v1.desktop_bundle"
SOURCE_POLICY_SCHEMA_VERSION = "automation.gui.permission_evidence.v1"

RAW_REJECTED_ARTIFACT_KINDS = {
    "broad_screenshot",
    "raw_accessibility_tree",
    "raw_stdout",
    "raw_stderr",
    "local_db",
    "full_consent_record",
    "raw_runtime_log",
    "provider_account_data",
}

WINDOWSERVER_READINESS_RECEIPT_ID = "windowserver-readiness-receipt"
WINDOWSERVER_CLEANUP_RECEIPT_ID = "windowserver-cleanup-receipt"
WINDOWSERVER_REASON_CODES = {
    "console_session_unavailable",
    "gui_session_unavailable",
    "hardware_model_unavailable",
    "screen_locked",
    "unsupported_platform",
    "virtualized_hardware",
    "windowserver_unavailable",
}
WINDOWSERVER_UNAVAILABLE_REASONS = {
    "console_session_unavailable",
    "gui_session_unavailable",
    "screen_locked",
    "windowserver_unavailable",
}
WINDOWSERVER_IDENTITY_FAILURE_REASONS = {
    "hardware_model_unavailable",
    "unsupported_platform",
    "virtualized_hardware",
}

PATTERNS: dict[str, re.Pattern[str]] = {
    "email": re.compile(r"\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b", re.IGNORECASE),
    "phone": re.compile(r"\b(?:\+?\d{1,3}[-. ]?)?(?:\(?\d{3}\)?[-. ]?)\d{3}[-. ]?\d{4}\b"),
    "credit_card": re.compile(r"\b(?:\d[ -]*?){13,19}\b"),
    "ssn": re.compile(r"\b\d{3}-\d{2}-\d{4}\b"),
    "iban": re.compile(r"\b[A-Z]{2}\d{2}[A-Z0-9 ]{11,30}\b"),
    "api_key": re.compile(
        r"(?i)\b(?:api[_-]?key|access[_-]?token|secret|password)\s*[:=]\s*[^\s]+|\b(?:sk|gho|xoxb)-[A-Za-z0-9_-]{6,}\b"
    ),
    "oauth_token": re.compile(r"\b(?:ya29\.|gho_|ghp_|xox[baprs]-)[A-Za-z0-9_-]+\b"),
    "user_path": re.compile(r"(?i)(?:[A-Z]:\\Users\\[^\\\s]+|/Users/[^\s/]+|/home/[^\s/]+)"),
    "provider_account": re.compile(r"(?i)\b(?:org|acct|account|workspace|tenant)[_-][A-Za-z0-9_-]{3,}\b"),
    "url_query_token": re.compile(r"(?i)[?&](?:token|access_token|api_key|apikey|code|secret)=[^&\s]+"),
    "credential": re.compile(r"(?i)\b(?:password|passcode|credential|security prompt)\b"),
    "sensitive_app": re.compile(r"(?i)\b(?:1Password|Bitwarden|Bank|Authenticator)\b"),
}


def _utc_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def _is_sha(value: str) -> bool:
    return bool(re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", value))


def _is_artifact_checksum(value: str) -> bool:
    return bool(re.fullmatch(r"sha256:[0-9a-f]{64}", value))


def _marker_counts(content: str) -> dict[str, int]:
    counts: dict[str, int] = {}
    for marker, pattern in PATTERNS.items():
        matches = pattern.findall(content)
        if matches:
            counts[marker] = len(matches)
    return counts


def _artifact_summary(content: str, marker_counts: dict[str, int]) -> dict[str, Any]:
    line_count = 0 if not content else content.count("\n") + 1
    return {
        "line_count": line_count,
        "character_count": len(content),
        "marker_counts": marker_counts,
    }


def _receipt_object(content: str, artifact_id: str) -> dict[str, Any]:
    if not content:
        raise ValueError(f"{artifact_id}:missing_receipt")
    try:
        payload = json.loads(content)
    except json.JSONDecodeError as exc:
        raise ValueError(f"{artifact_id}:invalid_json") from exc
    if not isinstance(payload, dict):
        raise ValueError(f"{artifact_id}:receipt_must_be_object")
    return payload


def _windowserver_readiness_projection(content: str) -> dict[str, Any]:
    artifact_id = WINDOWSERVER_READINESS_RECEIPT_ID
    payload = _receipt_object(content, artifact_id)
    required = {
        "schema_version",
        "probe",
        "dedicated_config",
        "hardware_isolation",
        "expected_state",
        "observed_state",
        "observed_as_expected",
        "reason_codes",
    }
    if set(payload) != required:
        raise ValueError(f"{artifact_id}:fields_do_not_match_contract")
    if type(payload["schema_version"]) is not int or payload["schema_version"] != 1:
        raise ValueError(f"{artifact_id}:unsupported_schema_version")
    if payload["probe"] != "maekon-macos-windowserver-readiness":
        raise ValueError(f"{artifact_id}:unsupported_probe")
    if payload["dedicated_config"] != "pass":
        raise ValueError(f"{artifact_id}:dedicated_config_not_verified")
    if payload["hardware_isolation"] != "pass":
        raise ValueError(f"{artifact_id}:hardware_isolation_not_verified")

    expected_state = payload["expected_state"]
    observed_state = payload["observed_state"]
    observed_as_expected = payload["observed_as_expected"]
    reason_codes = payload["reason_codes"]
    if expected_state not in {"ready", "unavailable"}:
        raise ValueError(f"{artifact_id}:unsupported_expected_state")
    if observed_state not in {"ready", "unavailable"}:
        raise ValueError(f"{artifact_id}:unsupported_observed_state")
    if observed_as_expected is not True:
        raise ValueError(f"{artifact_id}:control_not_observed_as_expected")
    if not isinstance(reason_codes, list) or any(
        not isinstance(code, str) or code not in WINDOWSERVER_REASON_CODES
        for code in reason_codes
    ):
        raise ValueError(f"{artifact_id}:unsupported_reason_codes")
    if len(reason_codes) != len(set(reason_codes)) or reason_codes != sorted(reason_codes):
        raise ValueError(f"{artifact_id}:reason_codes_not_canonical")
    if set(reason_codes) & WINDOWSERVER_IDENTITY_FAILURE_REASONS:
        raise ValueError(f"{artifact_id}:hardware_isolation_is_contradictory")
    unavailable_reasons = set(reason_codes) & WINDOWSERVER_UNAVAILABLE_REASONS
    if expected_state == "ready":
        if observed_state != "ready" or unavailable_reasons:
            raise ValueError(f"{artifact_id}:ready_control_is_contradictory")
    elif observed_state != "unavailable" or not unavailable_reasons:
        raise ValueError(f"{artifact_id}:unavailable_control_is_contradictory")

    return {
        "schema_version": 1,
        "probe": payload["probe"],
        "dedicated_config": "pass",
        "hardware_isolation": "pass",
        "expected_state": expected_state,
        "observed_state": observed_state,
        "observed_as_expected": True,
        "reason_codes": reason_codes,
    }


def _windowserver_cleanup_projection(content: str) -> dict[str, Any]:
    artifact_id = WINDOWSERVER_CLEANUP_RECEIPT_ID
    payload = _receipt_object(content, artifact_id)
    required = {
        "schema_version",
        "cleanup_status",
        "profile_absent",
        "process_absent",
        "tcc_mutation",
    }
    if set(payload) != required:
        raise ValueError(f"{artifact_id}:fields_do_not_match_contract")
    if type(payload["schema_version"]) is not int or payload["schema_version"] != 1:
        raise ValueError(f"{artifact_id}:unsupported_schema_version")
    if payload["cleanup_status"] != "pass":
        raise ValueError(f"{artifact_id}:cleanup_not_verified")
    if payload["profile_absent"] is not True:
        raise ValueError(f"{artifact_id}:profile_cleanup_not_verified")
    if payload["process_absent"] is not True:
        raise ValueError(f"{artifact_id}:process_cleanup_not_verified")
    if payload["tcc_mutation"] != "not_performed":
        raise ValueError(f"{artifact_id}:tcc_mutation_boundary_not_verified")
    return {
        "schema_version": 1,
        "cleanup_status": "pass",
        "profile_absent": True,
        "process_absent": True,
        "tcc_mutation": "not_performed",
    }


def _structured_evidence(artifact_id: str, content: str) -> dict[str, Any] | None:
    if artifact_id == WINDOWSERVER_READINESS_RECEIPT_ID:
        return _windowserver_readiness_projection(content)
    if artifact_id == WINDOWSERVER_CLEANUP_RECEIPT_ID:
        return _windowserver_cleanup_projection(content)
    return None


def _validate_metadata(
    *,
    commit_sha: str,
    release_tag: str,
    artifact_checksum: str,
    runner_label: str,
    cleanup_status: str,
) -> list[str]:
    errors: list[str] = []
    if not _is_sha(commit_sha) or len(commit_sha) != 40:
        errors.append("commit_sha must be a 40-character git SHA")
    if not release_tag.startswith("v"):
        errors.append("release_tag must start with v")
    if not _is_artifact_checksum(artifact_checksum):
        errors.append("artifact_checksum must be sha256:<64 lowercase hex>")
    if not runner_label:
        errors.append("runner_label is required")
    if cleanup_status not in {"pass", "blocked", "failed", "manual_required"}:
        errors.append("cleanup_status is unsupported")
    elif cleanup_status != "pass":
        errors.append("cleanup_not_verified")
    return errors


def sanitize_bundle(
    *,
    inputs: list[dict[str, Any]],
    output_dir: Path,
    commit_sha: str,
    release_tag: str,
    artifact_checksum: str,
    runner_label: str,
    cleanup_status: str,
    retention_days: int = 7,
    generated_at: str | None = None,
) -> dict[str, Any]:
    output_dir.mkdir(parents=True, exist_ok=True)
    errors = _validate_metadata(
        commit_sha=commit_sha,
        release_tag=release_tag,
        artifact_checksum=artifact_checksum,
        runner_label=runner_label,
        cleanup_status=cleanup_status,
    )
    artifacts: list[dict[str, Any]] = []
    blocked = bool(errors)
    artifact_ids = {str(item.get("id") or f"artifact-{index}") for index, item in enumerate(inputs)}
    windowserver_receipt_ids = {
        WINDOWSERVER_READINESS_RECEIPT_ID,
        WINDOWSERVER_CLEANUP_RECEIPT_ID,
    }
    if artifact_ids & windowserver_receipt_ids:
        for missing_id in sorted(windowserver_receipt_ids - artifact_ids):
            blocked = True
            errors.append(f"{missing_id}:missing_receipt")

    for index, item in enumerate(inputs):
        artifact_id = str(item.get("id") or f"artifact-{index}")
        artifact_kind = str(item.get("artifact_kind") or "log_excerpt")
        source_path = str(item.get("path") or artifact_id)
        content = str(item.get("content") or "")

        if artifact_kind in RAW_REJECTED_ARTIFACT_KINDS:
            blocked = True
            artifacts.append(
                {
                    "id": artifact_id,
                    "artifact_kind": artifact_kind,
                    "source_path_class": Path(source_path).suffix.lower().lstrip(".") or "unknown",
                    "privacy_status": "rejected",
                    "redaction_status": "rejected",
                    "retention_days": retention_days,
                    "blocked_reason": "raw_artifact_kind_rejected",
                    "sanitized": False,
                    "commit_sha": commit_sha,
                    "release_tag": release_tag,
                    "artifact_checksum": artifact_checksum,
                    "runner_label": runner_label,
                    "cleanup_status": cleanup_status,
                }
            )
            continue

        counts = _marker_counts(content)
        markers = sorted(counts)
        structured_evidence = None
        try:
            structured_evidence = _structured_evidence(artifact_id, content)
        except ValueError as exc:
            blocked = True
            errors.append(str(exc))
        artifact_name = f"{artifact_id}.sanitized.json"
        sanitized_record = {
            "id": artifact_id,
            "artifact_kind": artifact_kind,
            "source_path_class": Path(source_path).suffix.lower().lstrip(".") or "text",
            "privacy_status": "redacted" if markers else "safe",
            "redaction_status": "redacted" if markers else "safe",
            "retention_days": retention_days,
            "markers": markers,
            "marker_counts": counts,
            "summary": _artifact_summary(content, counts),
            "sensitive_app_excluded": "sensitive_app" in counts,
            "sanitized": True,
            "sanitized_path": artifact_name,
            "commit_sha": commit_sha,
            "release_tag": release_tag,
            "artifact_checksum": artifact_checksum,
            "runner_label": runner_label,
            "cleanup_status": cleanup_status,
        }
        if structured_evidence is not None:
            sanitized_record["structured_evidence"] = structured_evidence
        (output_dir / artifact_name).write_text(
            json.dumps(sanitized_record, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        artifacts.append(sanitized_record)

    privacy_status = "rejected" if blocked else ("redacted" if any(a.get("privacy_status") == "redacted" for a in artifacts) else "safe")
    redaction_status = "rejected" if blocked else ("redacted" if any(a.get("redaction_status") == "redacted" for a in artifacts) else "safe")
    bundle = {
        "schema_version": SCHEMA_VERSION,
        "source_policy_schema_version": SOURCE_POLICY_SCHEMA_VERSION,
        "generated_at": generated_at or _utc_now(),
        "privacy_status": privacy_status,
        "redaction_status": redaction_status,
        "release_decision_state": "blocked_for_privacy" if blocked else "optional",
        "commit_sha": commit_sha,
        "release_tag": release_tag,
        "artifact_checksum": artifact_checksum,
        "runner_label": runner_label,
        "cleanup_status": cleanup_status,
        "retention_days": retention_days,
        "errors": errors,
        "artifacts": artifacts,
    }

    output_name = "blocked-report.json" if blocked else "manifest.json"
    (output_dir / output_name).write_text(
        json.dumps(bundle, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return bundle


def _read_input(path: Path) -> str:
    if not path.is_file():
        return ""
    return path.read_text(encoding="utf-8", errors="replace")


def _cmd_sanitize(args: argparse.Namespace) -> int:
    inputs = []
    for index, input_path in enumerate(args.input):
        path = Path(input_path)
        inputs.append(
            {
                "id": args.input_id[index] if index < len(args.input_id) else path.stem,
                "path": str(path),
                "artifact_kind": args.artifact_kind,
                "content": _read_input(path),
            }
        )
    bundle = sanitize_bundle(
        inputs=inputs,
        output_dir=Path(args.output_dir),
        commit_sha=args.commit_sha,
        release_tag=args.release_tag,
        artifact_checksum=args.artifact_checksum,
        runner_label=args.runner_label,
        cleanup_status=args.cleanup_status,
        retention_days=args.retention_days,
    )
    print(json.dumps({"output_dir": args.output_dir, "privacy_status": bundle["privacy_status"]}, sort_keys=True))
    return 1 if bundle["release_decision_state"] == "blocked_for_privacy" else 0


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Sanitize Maekon E19 desktop smoke evidence bundles.")
    parser.add_argument("sanitize", nargs="?")
    parser.add_argument("--input", action="append", default=[])
    parser.add_argument("--input-id", action="append", default=[])
    parser.add_argument("--artifact-kind", default="log_excerpt")
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--commit-sha", required=True)
    parser.add_argument("--release-tag", required=True)
    parser.add_argument("--artifact-checksum", required=True)
    parser.add_argument("--runner-label", required=True)
    parser.add_argument("--cleanup-status", required=True)
    parser.add_argument("--retention-days", type=int, default=7)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    if not args.input:
        print("at least one --input is required", file=sys.stderr)
        return 2
    return _cmd_sanitize(args)


if __name__ == "__main__":
    raise SystemExit(main())
