#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA_VERSION = "automation.gui.benchmark_report.v1.release_decision"
SOURCE_REPORT_SCHEMA_VERSION = "automation.gui.benchmark_report.v1"
EVIDENCE_POLICY_SCHEMA_VERSION = "automation.gui.permission_evidence.v1"
CHECKLIST_REGISTRY_SCHEMA_VERSION = "maekon.release_checklist_dispositions.v2"
CHECKLIST_RESULTS_SCHEMA_VERSION = "maekon.release_checklist_results.v2"

CLIENT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CHECKLIST_PATH = CLIENT_ROOT / "docs" / "release-checklist.md"
DEFAULT_CHECKLIST_REGISTRY_PATH = (
    CLIENT_ROOT / "docs" / "contracts" / "release-checklist-dispositions.v2.json"
)
CHECKLIST_ID_PATTERN = re.compile(r"<!-- release-check-id: ([A-Z0-9-]+) -->")

DECISION_STATES = {
    "pass",
    "optional",
    "soft_block",
    "hard_block",
    "blocked_for_privacy",
}
SAFE_PRIVACY_STATUSES = {"safe", "redacted"}
SAFE_REDACTION_STATUSES = {"safe", "redacted"}
HISTORY_STAGES = ("initial", "pivot", "current")
CHECKLIST_PHASES = {"pre_publish", "post_publish"}


def _load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def _sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _load_checklist_items(path: Path) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    pending: tuple[str, int] | None = None

    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        match = CHECKLIST_ID_PATTERN.fullmatch(line)
        if match:
            if pending is not None:
                raise ValueError(f"checklist id {pending[0]} is not attached to an item")
            pending = (match.group(1), line_number)
            continue

        if not line.startswith("- [ ] "):
            continue
        if pending is None:
            raise ValueError(f"checklist item at line {line_number} is missing a stable id")

        item_id, id_line = pending
        items.append(
            {
                "id": item_id,
                "line": line_number,
                "id_line": id_line,
                "summary": line.removeprefix("- [ ] ").strip(),
            }
        )
        pending = None

    if pending is not None:
        raise ValueError(f"checklist id {pending[0]} is not attached to an item")
    if not items:
        raise ValueError("release checklist must contain stable items")

    ids = [item["id"] for item in items]
    duplicates = sorted({item_id for item_id in ids if ids.count(item_id) > 1})
    if duplicates:
        raise ValueError(f"duplicate checklist ids: {', '.join(duplicates)}")
    return items


def _registry_blockers(
    registry: Any,
    *,
    checklist_items: list[dict[str, Any]],
) -> list[str]:
    errors: list[str] = []
    if not isinstance(registry, dict):
        return ["checklist disposition registry must be an object"]
    if registry.get("schema_version") != CHECKLIST_REGISTRY_SCHEMA_VERSION:
        errors.append(f"checklist registry must use {CHECKLIST_REGISTRY_SCHEMA_VERSION}")
    if registry.get("checklist_path") != "docs/release-checklist.md":
        errors.append("checklist registry path must be docs/release-checklist.md")
    default_phase = registry.get("default_phase")
    if default_phase not in CHECKLIST_PHASES:
        errors.append("checklist registry default_phase must be pre_publish or post_publish")

    registry_items = registry.get("items")
    if not isinstance(registry_items, list):
        return errors + ["checklist registry items must be a list"]

    expected_ids = [item["id"] for item in checklist_items]
    actual_ids = [item.get("id") for item in registry_items if isinstance(item, dict)]
    if len(actual_ids) != len(set(actual_ids)):
        errors.append("checklist registry ids must be unique")
    if actual_ids != expected_ids:
        missing = [item_id for item_id in expected_ids if item_id not in actual_ids]
        unknown = [item_id for item_id in actual_ids if item_id not in expected_ids]
        if missing:
            errors.append(f"checklist registry missing ids: {', '.join(missing)}")
        if unknown:
            errors.append(f"checklist registry has unknown ids: {', '.join(unknown)}")
        if not missing and not unknown:
            errors.append("checklist registry order must match the canonical checklist")

    allowed_subject_kinds = {
        "machine": {"command", "lane"},
        "evidence": {"command", "evidence", "lane"},
        "human": {"human"},
    }
    for index, item in enumerate(registry_items):
        prefix = f"checklist registry item[{index}]"
        if not isinstance(item, dict):
            errors.append(f"{prefix} must be an object")
            continue
        disposition = item.get("disposition")
        if disposition not in allowed_subject_kinds:
            errors.append(f"{prefix} has unsupported disposition: {disposition}")
            continue
        subject = item.get("subject")
        if not isinstance(subject, dict):
            errors.append(f"{prefix} subject is required")
            continue
        if subject.get("kind") not in allowed_subject_kinds[disposition]:
            errors.append(f"{prefix} subject kind does not match {disposition}")
        if not subject.get("ref"):
            errors.append(f"{prefix} subject ref is required")
        if not isinstance(subject.get("available"), bool):
            errors.append(f"{prefix} subject availability is required")
        elif subject.get("available") is False and not subject.get("unavailable_reason"):
            errors.append(f"{prefix} unavailable subject requires a reason")
        if disposition == "human" and not item.get("why"):
            errors.append(f"{prefix} human disposition requires why")
        phase = item.get("phase", default_phase)
        if phase not in CHECKLIST_PHASES:
            errors.append(f"{prefix} has unsupported phase: {phase}")
    return errors


def _build_checklist_record(
    *,
    checklist_path: Path,
    registry_path: Path,
    results: Any,
) -> dict[str, Any]:
    try:
        checklist_items = _load_checklist_items(checklist_path)
    except ValueError as exc:
        raise SystemExit(f"release checklist contract error: {exc}") from exc

    registry = _load_json(registry_path)
    registry_errors = _registry_blockers(registry, checklist_items=checklist_items)
    if registry_errors:
        raise SystemExit("release checklist contract error: " + "; ".join(registry_errors))

    if not isinstance(results, dict) or results.get("schema_version") != CHECKLIST_RESULTS_SCHEMA_VERSION:
        raise SystemExit(f"checklist results must use {CHECKLIST_RESULTS_SCHEMA_VERSION}")
    result_items = results.get("items")
    if not isinstance(result_items, list):
        raise SystemExit("checklist results items must be a list")

    expected_ids = [item["id"] for item in checklist_items]
    actual_ids = [item.get("id") for item in result_items if isinstance(item, dict)]
    if len(actual_ids) != len(set(actual_ids)):
        raise SystemExit("checklist result ids must be unique")
    missing = [item_id for item_id in expected_ids if item_id not in actual_ids]
    unknown = [item_id for item_id in actual_ids if item_id not in expected_ids]
    if missing or unknown or actual_ids != expected_ids:
        details = []
        if missing:
            details.append(f"missing ids: {', '.join(missing)}")
        if unknown:
            details.append(f"unknown ids: {', '.join(unknown)}")
        if not missing and not unknown:
            details.append("result order must match the canonical checklist")
        raise SystemExit("checklist results do not cover the canonical checklist: " + "; ".join(details))

    registry_by_id = {item["id"]: item for item in registry["items"]}
    combined: list[dict[str, Any]] = []
    for result in result_items:
        item_id = result["id"]
        registered = registry_by_id[item_id]
        phase = registered.get("phase", registry["default_phase"])
        state = result.get("state")
        allowed_states = (
            {"pass", "blocked", "pending"}
            if phase == "post_publish"
            else {"pass", "blocked"}
        )
        if state not in allowed_states:
            raise SystemExit(f"checklist result {item_id} has unsupported state: {state}")
        if not result.get("receipt"):
            raise SystemExit(f"checklist result {item_id} requires a receipt")

        if registered["disposition"] == "human" and state == "pass" and not result.get("reviewer"):
            raise SystemExit(f"human checklist result {item_id} requires a reviewer")

        combined.append(
            {
                "id": item_id,
                "disposition": registered["disposition"],
                "phase": phase,
                "result": {
                    "state": state,
                    "receipt": result["receipt"],
                    **({"reviewer": result["reviewer"]} if result.get("reviewer") else {}),
                },
            }
        )

    state_counts = {
        state: sum(1 for item in combined if item["result"]["state"] == state)
        for state in ("pass", "blocked", "pending")
    }
    return {
        "schema_version": CHECKLIST_RESULTS_SCHEMA_VERSION,
        "source_path": "docs/release-checklist.md",
        "source_sha256": _sha256_file(checklist_path),
        "registry_schema_version": CHECKLIST_REGISTRY_SCHEMA_VERSION,
        "registry_sha256": _sha256_file(registry_path),
        "item_count": len(combined),
        "summary": state_counts,
        "items": combined,
    }


def _load_benchmark_report(path: Path) -> Any:
    """벤치마크 리포트를 읽되, 감싸인 형태를 그 자리에서 잡아낸다.

    E19 워크플로가 업로드하는 `windows-gui-session-benchmark.json` 은 리포트가
    아니라 `{"ok": ..., "report": {...}}` 래퍼다. **두 층 모두 같은
    `schema_version` 을 들고 있어서** 래퍼를 넘겨도 스키마 검사는 통과하고,
    거부는 저 아래 "results must not be empty" 로 나온다. 그 문구는 벤치마크가
    아무것도 실행하지 못한 것처럼 읽혀서, 실제로는 인자를 잘못 준 것인데
    리포트 내용을 의심하게 만든다. 2026-08-21 rc.9 에서 그 오독으로 시간을 썼다.

    모양 문제는 모양 문제라고 말한다.
    """
    payload = _load_json(path)
    if not isinstance(payload, dict):
        return payload
    if payload.get("schema_version") == SOURCE_REPORT_SCHEMA_VERSION:
        return payload
    inner = payload.get("report")
    if isinstance(inner, dict) and inner.get("schema_version") == SOURCE_REPORT_SCHEMA_VERSION:
        raise SystemExit(
            f"{path} is a wrapper, not a benchmark report: the "
            f"{SOURCE_REPORT_SCHEMA_VERSION} object is nested under its 'report' key. "
            "Pass that inner object (e.g. `jq .report` into a temporary file)."
        )
    return payload


def _parse_utc(value: str) -> datetime:
    if not value.endswith("Z"):
        raise ValueError(f"timestamp must be UTC/Z: {value}")
    parsed = datetime.fromisoformat(value.removesuffix("Z") + "+00:00")
    return parsed.astimezone(timezone.utc)


def _now_utc() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def _is_sha256(value: str) -> bool:
    return bool(re.fullmatch(r"[0-9a-f]{64}", value))


def _is_git_sha(value: str) -> bool:
    return bool(re.fullmatch(r"[0-9a-f]{40}", value))


def _artifact_sha(value: str) -> str:
    return value.removeprefix("sha256:")


def _report_blockers(
    report: dict[str, Any],
    *,
    now: str | None,
    max_age_seconds: int,
) -> list[str]:
    errors: list[str] = []

    if report.get("schema_version") != SOURCE_REPORT_SCHEMA_VERSION:
        errors.append("benchmark report must use automation.gui.benchmark_report.v1")

    generated_at = report.get("generated_at")
    if not isinstance(generated_at, str):
        errors.append("benchmark report generated_at is required")
    else:
        try:
            generated = _parse_utc(generated_at)
            current = _parse_utc(now) if now else datetime.now(timezone.utc)
            age_seconds = (current - generated).total_seconds()
            if age_seconds < 0:
                errors.append("benchmark report generated_at cannot be in the future")
            if age_seconds > max_age_seconds:
                errors.append("benchmark report is stale")
        except ValueError as exc:
            errors.append(f"benchmark report {exc}")

    results = report.get("results")
    if not isinstance(results, list) or not results:
        errors.append("benchmark report results must not be empty")
        return errors

    for index, entry in enumerate(results):
        result = entry.get("result") if isinstance(entry, dict) else None
        if not isinstance(result, dict):
            errors.append(f"result[{index}] is missing result object")
            continue

        case_id = result.get("case_id", f"result[{index}]")
        outcome = result.get("outcome")
        privacy_status = result.get("privacy_status")
        evidence_paths = result.get("evidence_paths") or []
        evidence_artifacts = result.get("evidence_artifacts") or []

        if privacy_status not in SAFE_PRIVACY_STATUSES:
            errors.append(f"privacy status is not shareable for {case_id}: {privacy_status}")

        if outcome == "pass":
            if entry.get("evidence_fresh") is not True:
                errors.append(f"stale evidence cannot pass for {case_id}")
            if not evidence_paths:
                errors.append(f"pass result is missing evidence paths for {case_id}")
            if not evidence_artifacts:
                errors.append(f"pass result is missing evidence artifacts for {case_id}")
            if result.get("input_execution_mode") in {"noop", "dry_run_worker", "unsupported"}:
                errors.append(f"dispatch-only input mode cannot pass for {case_id}")
            if result.get("verification_mode") == "command_accepted":
                errors.append(f"command_accepted alone cannot pass for {case_id}")
        elif outcome in {"fail", "blocked"}:
            errors.append(f"blocking smoke outcome for {case_id}: {outcome}")
        elif outcome in {"degraded", "unsupported", "skip"}:
            errors.append(f"non-passing smoke outcome for {case_id}: {outcome}")

    return errors


def _evidence_artifact_blockers(evidence_artifacts: list[dict[str, Any]]) -> list[str]:
    errors: list[str] = []

    if not evidence_artifacts:
        errors.append("evidence_artifacts must not be empty")
        return errors

    for index, artifact in enumerate(evidence_artifacts):
        prefix = f"evidence_artifacts[{index}]"
        for key in ("id", "path", "sha256", "privacy_status", "redaction_status", "sanitized"):
            if key not in artifact:
                errors.append(f"{prefix} missing {key}")

        sha256 = artifact.get("sha256")
        if isinstance(sha256, str) and not _is_sha256(sha256):
            errors.append(f"{prefix} has invalid sha256")

        if artifact.get("privacy_status") not in SAFE_PRIVACY_STATUSES:
            errors.append(f"{prefix} privacy status is not shareable")
        if artifact.get("redaction_status") not in SAFE_REDACTION_STATUSES:
            errors.append(f"{prefix} redaction status is not shareable")
        if artifact.get("sanitized") is not True:
            errors.append(f"{prefix} sanitized must be true")

    return errors


def _claim_blockers(claims: list[dict[str, Any]]) -> list[str]:
    errors: list[str] = []

    if not claims:
        errors.append("release-critical claims must not be empty")
        return errors

    for index, claim in enumerate(claims):
        prefix = f"claims[{index}]"
        state = claim.get("state")
        if state not in DECISION_STATES:
            errors.append(f"{prefix} has unsupported state: {state}")

        evidence = claim.get("evidence")
        if not isinstance(evidence, dict):
            errors.append(f"{prefix} evidence mapping is missing")
            continue

        history_relevant = claim.get("history_relevant") is True
        if history_relevant:
            required_stages = HISTORY_STAGES
        else:
            required_stages = ("current",)
            if not claim.get("history_not_relevant_reason"):
                errors.append(f"{prefix} must explain why initial/pivot history is not relevant")

        for stage in required_stages:
            stage_evidence = evidence.get(stage)
            if not isinstance(stage_evidence, dict):
                errors.append(f"{prefix} history evidence missing: {stage}")
                continue

            for key in ("sha", "date", "path", "summary"):
                if not stage_evidence.get(key):
                    errors.append(f"{prefix}.{stage} missing {key}")

            sha = stage_evidence.get("sha")
            if isinstance(sha, str) and not (_is_git_sha(sha) or _is_sha256(sha)):
                errors.append(f"{prefix}.{stage} has invalid sha")

            date = stage_evidence.get("date")
            if isinstance(date, str) and not re.fullmatch(r"\d{4}-\d{2}-\d{2}", date):
                errors.append(f"{prefix}.{stage} has invalid date")

    return errors


def _checklist_blockers(
    checklist: Any,
    *,
    checklist_path: Path,
    checklist_registry_path: Path,
) -> list[str]:
    errors: list[str] = []
    if not isinstance(checklist, dict):
        return ["checklist results are required"]
    if checklist.get("schema_version") != CHECKLIST_RESULTS_SCHEMA_VERSION:
        errors.append(f"checklist results must use {CHECKLIST_RESULTS_SCHEMA_VERSION}")
    if checklist.get("source_path") != "docs/release-checklist.md":
        errors.append("checklist source path must be docs/release-checklist.md")
    if checklist.get("registry_schema_version") != CHECKLIST_REGISTRY_SCHEMA_VERSION:
        errors.append(f"checklist registry must use {CHECKLIST_REGISTRY_SCHEMA_VERSION}")

    try:
        canonical_items = _load_checklist_items(checklist_path)
        canonical_registry = _load_json(checklist_registry_path)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        return errors + [f"canonical checklist contract cannot be loaded: {exc}"]

    errors.extend(_registry_blockers(canonical_registry, checklist_items=canonical_items))
    if checklist.get("source_sha256") != _sha256_file(checklist_path):
        errors.append("checklist source hash does not match the canonical checklist")
    if checklist.get("registry_sha256") != _sha256_file(checklist_registry_path):
        errors.append("checklist registry hash does not match the canonical registry")

    items = checklist.get("items")
    if not isinstance(items, list):
        return errors + ["checklist items must be a list"]
    if checklist.get("item_count") != len(items):
        errors.append("checklist item_count must match checklist items")

    expected_ids = [item["id"] for item in canonical_items]
    actual_ids = [item.get("id") for item in items if isinstance(item, dict)]
    if len(actual_ids) != len(set(actual_ids)):
        errors.append("manifest checklist ids must be unique")
    if actual_ids != expected_ids:
        missing = [item_id for item_id in expected_ids if item_id not in actual_ids]
        unknown = [item_id for item_id in actual_ids if item_id not in expected_ids]
        if missing:
            errors.append(f"manifest checklist missing ids: {', '.join(missing)}")
        if unknown:
            errors.append(f"manifest checklist has unknown ids: {', '.join(unknown)}")
        if not missing and not unknown:
            errors.append("manifest checklist order must match the canonical checklist")

    registry_by_id = {item["id"]: item for item in canonical_registry.get("items", [])}
    state_counts = {"pass": 0, "blocked": 0, "pending": 0}
    for index, item in enumerate(items):
        prefix = f"checklist.items[{index}]"
        if not isinstance(item, dict):
            errors.append(f"{prefix} must be an object")
            continue
        item_id = item.get("id")
        registered = registry_by_id.get(item_id)
        if registered is None:
            continue
        if item.get("disposition") != registered.get("disposition"):
            errors.append(f"{prefix} disposition does not match the registry")
        default_phase = canonical_registry.get("default_phase")
        phase = registered.get("phase", default_phase)
        if item.get("phase", default_phase) != phase:
            errors.append(f"{prefix} phase does not match the registry")
        result = item.get("result")
        if not isinstance(result, dict):
            errors.append(f"{prefix} result is required")
            continue
        state = result.get("state")
        if state not in state_counts:
            errors.append(f"{prefix} has unsupported result state: {state}")
            continue
        state_counts[state] += 1
        if not result.get("receipt"):
            errors.append(f"{prefix} receipt is required")
        if registered.get("disposition") == "human" and state == "pass" and not result.get("reviewer"):
            errors.append(f"{prefix} human pass requires a reviewer")
        if registered.get("subject", {}).get("available") is False:
            reason = registered["subject"].get("unavailable_reason", "reason missing")
            errors.append(f"checklist subject unavailable for {item_id}: {reason}")
        if state == "pending" and phase == "post_publish":
            continue
        if state != "pass":
            errors.append(f"checklist result blocks release for {item_id}: {state}")

    if checklist.get("summary") != state_counts:
        errors.append("checklist summary does not match item results")
    return errors


def _manifest_blockers(
    manifest: dict[str, Any],
    *,
    now: str | None = None,
    max_age_seconds: int = 3600,
    checklist_path: Path = DEFAULT_CHECKLIST_PATH,
    checklist_registry_path: Path = DEFAULT_CHECKLIST_REGISTRY_PATH,
) -> list[str]:
    errors: list[str] = []

    if manifest.get("schema_version") != SCHEMA_VERSION:
        errors.append(f"manifest must use {SCHEMA_VERSION}")
    if manifest.get("source_report_schema_version") != SOURCE_REPORT_SCHEMA_VERSION:
        errors.append(f"source report schema must be {SOURCE_REPORT_SCHEMA_VERSION}")
    if manifest.get("evidence_policy_schema_version") != EVIDENCE_POLICY_SCHEMA_VERSION:
        errors.append(f"evidence policy schema must be {EVIDENCE_POLICY_SCHEMA_VERSION}")

    commit_sha = manifest.get("commit_sha")
    if not isinstance(commit_sha, str) or not _is_git_sha(commit_sha):
        errors.append("commit_sha must be a 40-character git SHA")

    release_tag = manifest.get("release_tag")
    if not isinstance(release_tag, str) or not release_tag.startswith("v"):
        errors.append("release_tag must start with v")

    artifact_checksum = manifest.get("artifact_checksum")
    if not isinstance(artifact_checksum, str) or not artifact_checksum.startswith("sha256:") or not _is_sha256(_artifact_sha(artifact_checksum)):
        errors.append("artifact_checksum must be sha256:<64 lowercase hex>")

    generated_at = manifest.get("generated_at")
    if not isinstance(generated_at, str):
        errors.append("generated_at is required")
    else:
        try:
            generated = _parse_utc(generated_at)
            current = _parse_utc(now) if now else datetime.now(timezone.utc)
            age_seconds = (current - generated).total_seconds()
            if age_seconds < 0:
                errors.append("generated_at cannot be in the future")
            if age_seconds > max_age_seconds:
                errors.append("manifest evidence is stale")
        except ValueError as exc:
            errors.append(str(exc))

    run = manifest.get("run")
    if not isinstance(run, dict):
        errors.append("run metadata is required")
    else:
        if not (run.get("workflow_run_url") or run.get("manual_evidence_id")):
            errors.append("run requires workflow_run_url or manual_evidence_id")
        runner = run.get("runner")
        if not isinstance(runner, dict):
            errors.append("runner metadata is required")
        else:
            if not runner.get("os"):
                errors.append("runner os is required")
            if not runner.get("labels"):
                errors.append("runner labels are required")

    if manifest.get("privacy_status") not in SAFE_PRIVACY_STATUSES:
        errors.append("manifest privacy status is not shareable")
    if manifest.get("redaction_status") not in SAFE_REDACTION_STATUSES:
        errors.append("manifest redaction status is not shareable")

    cleanup = manifest.get("cleanup")
    if not isinstance(cleanup, dict) or cleanup.get("status") != "pass":
        errors.append("cleanup result must be pass")

    issues = manifest.get("covered_issue_numbers")
    if not isinstance(issues, list) or not issues or not all(isinstance(issue, int) for issue in issues):
        errors.append("covered_issue_numbers must contain issue numbers")

    report = manifest.get("benchmark_report")
    if not isinstance(report, dict):
        errors.append("benchmark_report is required")
    else:
        errors.extend(
            _report_blockers(
                report,
                now=now,
                max_age_seconds=max_age_seconds,
            )
        )

    artifacts = manifest.get("evidence_artifacts")
    if not isinstance(artifacts, list):
        errors.append("evidence_artifacts must be a list")
    else:
        errors.extend(_evidence_artifact_blockers(artifacts))

    claims = manifest.get("claims")
    if not isinstance(claims, list):
        errors.append("claims must be a list")
    else:
        errors.extend(_claim_blockers(claims))

    errors.extend(
        _checklist_blockers(
            manifest.get("checklist"),
            checklist_path=checklist_path,
            checklist_registry_path=checklist_registry_path,
        )
    )

    decision = manifest.get("release_decision")
    if not isinstance(decision, dict):
        errors.append("release_decision is required")
    elif decision.get("state") not in DECISION_STATES:
        errors.append(f"release_decision state is unsupported: {decision.get('state')}")

    return errors


def _derive_privacy_status(report: dict[str, Any], evidence_artifacts: list[dict[str, Any]]) -> str:
    report_statuses = [
        ((entry.get("result") or {}).get("privacy_status"))
        for entry in report.get("results", [])
        if isinstance(entry, dict)
    ]
    artifact_statuses = [artifact.get("privacy_status") for artifact in evidence_artifacts]
    statuses = report_statuses + artifact_statuses
    if any(status not in SAFE_PRIVACY_STATUSES for status in statuses):
        return "rejected"
    if "redacted" in statuses:
        return "redacted"
    return "safe"


def _derive_redaction_status(evidence_artifacts: list[dict[str, Any]]) -> str:
    if any(artifact.get("sanitized") is not True for artifact in evidence_artifacts):
        return "raw"
    if any(artifact.get("redaction_status") == "redacted" for artifact in evidence_artifacts):
        return "redacted"
    return "safe"


def _derive_decision(errors: list[str]) -> str:
    if any("privacy" in error or "redaction" in error or "sanitized" in error for error in errors):
        return "blocked_for_privacy"
    if errors:
        return "hard_block"
    return "pass"


def build_manifest(
    *,
    benchmark_report_path: Path,
    release_tag: str,
    commit_sha: str,
    artifact_checksum: str,
    workflow_run_url: str | None,
    manual_evidence_id: str | None,
    runner_os: str,
    runner_labels: list[str],
    evidence_artifacts: list[dict[str, Any]],
    claims: list[dict[str, Any]],
    checklist_results: dict[str, Any],
    covered_issue_numbers: list[int],
    cleanup_status: str,
    cleanup_summary: str,
    generated_at: str | None = None,
    checklist_path: Path = DEFAULT_CHECKLIST_PATH,
    checklist_registry_path: Path = DEFAULT_CHECKLIST_REGISTRY_PATH,
) -> dict[str, Any]:
    report = _load_benchmark_report(benchmark_report_path)
    checklist = _build_checklist_record(
        checklist_path=checklist_path,
        registry_path=checklist_registry_path,
        results=checklist_results,
    )
    manifest = {
        "schema_version": SCHEMA_VERSION,
        "source_report_schema_version": SOURCE_REPORT_SCHEMA_VERSION,
        "evidence_policy_schema_version": EVIDENCE_POLICY_SCHEMA_VERSION,
        "generated_at": generated_at or _now_utc(),
        "commit_sha": commit_sha,
        "release_tag": release_tag,
        "artifact_checksum": artifact_checksum,
        "run": {
            "workflow_run_url": workflow_run_url,
            "manual_evidence_id": manual_evidence_id,
            "runner": {
                "os": runner_os,
                "labels": runner_labels,
            },
        },
        "benchmark_report": report,
        "evidence_artifacts": evidence_artifacts,
        "privacy_status": _derive_privacy_status(report, evidence_artifacts),
        "redaction_status": _derive_redaction_status(evidence_artifacts),
        "cleanup": {
            "status": cleanup_status,
            "summary": cleanup_summary,
        },
        "covered_issue_numbers": covered_issue_numbers,
        "claims": claims,
        "checklist": checklist,
        "release_decision": {
            "state": "pass",
            "reasons": [],
        },
    }
    blockers = _manifest_blockers(
        manifest,
        now=generated_at,
        checklist_path=checklist_path,
        checklist_registry_path=checklist_registry_path,
    )
    manifest["release_decision"] = {
        "state": _derive_decision(blockers),
        "reasons": blockers,
    }
    return manifest


def validate_manifest(
    manifest: dict[str, Any],
    *,
    now: str | None = None,
    max_age_seconds: int = 3600,
    checklist_path: Path = DEFAULT_CHECKLIST_PATH,
    checklist_registry_path: Path = DEFAULT_CHECKLIST_REGISTRY_PATH,
) -> list[str]:
    return _manifest_blockers(
        manifest,
        now=now,
        max_age_seconds=max_age_seconds,
        checklist_path=checklist_path,
        checklist_registry_path=checklist_registry_path,
    )


def _parse_json_arg(value: str | None, default: Any) -> Any:
    if value is None:
        return default
    path = Path(value)
    if path.is_file():
        return _load_json(path)
    return json.loads(value)


def _cmd_build(args: argparse.Namespace) -> int:
    manifest = build_manifest(
        benchmark_report_path=Path(args.benchmark_report),
        release_tag=args.release_tag,
        commit_sha=args.commit_sha,
        artifact_checksum=args.artifact_checksum,
        workflow_run_url=args.workflow_run_url,
        manual_evidence_id=args.manual_evidence_id,
        runner_os=args.runner_os,
        runner_labels=args.runner_label,
        evidence_artifacts=_parse_json_arg(args.evidence_artifacts, []),
        claims=_parse_json_arg(args.claims, []),
        checklist_results=_parse_json_arg(args.checklist_results, {}),
        covered_issue_numbers=[int(issue) for issue in args.issue_number],
        cleanup_status=args.cleanup_status,
        cleanup_summary=args.cleanup_summary,
        generated_at=args.generated_at,
        checklist_path=Path(args.checklist),
        checklist_registry_path=Path(args.checklist_registry),
    )
    output = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    if args.output:
        Path(args.output).write_text(output, encoding="utf-8")
    else:
        print(output, end="")

    decision = manifest["release_decision"]
    if decision["state"] == "pass":
        return 0
    # `validate` 는 거부 사유를 stderr 로 찍는데 `build` 는 exit 1 만 냈다. 사유가
    # 결과 JSON 안에만 있으니, 막힌 이유를 알려면 build 한 뒤 validate 를 또 돌려야
    # 했다 — rc.9 에서 실제로 세 번 반복했다. 같은 실패는 같은 모양으로 보고한다.
    for reason in decision["reasons"]:
        print(f"release-decision manifest rejection: {reason}", file=sys.stderr)
    if args.output:
        print(
            f"release-decision manifest written to {args.output} in state "
            f"{decision['state']!r}; it will not pass validate",
            file=sys.stderr,
        )
    return 1


def _cmd_validate(args: argparse.Namespace) -> int:
    manifest = _load_json(Path(args.manifest))
    errors = validate_manifest(
        manifest,
        now=args.now,
        max_age_seconds=args.max_age_seconds,
        checklist_path=Path(args.checklist),
        checklist_registry_path=Path(args.checklist_registry),
    )
    if errors:
        for error in errors:
            print(f"release-decision manifest rejection: {error}", file=sys.stderr)
        return 1
    print("release-decision manifest accepted")
    return 0


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Build or validate Maekon E19 release-decision manifests.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    build = subparsers.add_parser("build")
    build.add_argument("--benchmark-report", required=True)
    build.add_argument("--release-tag", required=True)
    build.add_argument("--commit-sha", required=True)
    build.add_argument("--artifact-checksum", required=True)
    build.add_argument("--workflow-run-url")
    build.add_argument("--manual-evidence-id")
    build.add_argument("--runner-os", required=True)
    build.add_argument("--runner-label", action="append", default=[])
    # 둘 다 optional 로 보였지만 validate 는 빈 값을 언제나 거부한다
    # ("evidence_artifacts must not be empty" / "release-critical claims must not
    # be empty"). optional 처럼 생긴 필수 인자는 한 번에 하나씩만 알려주므로
    # 왕복이 쌓인다 — 서명 없이 만든 태그를 되돌리던 rc.9 에서 그 왕복이 비쌌다.
    build.add_argument("--evidence-artifacts", required=True)
    build.add_argument("--claims", required=True)
    build.add_argument("--checklist-results", required=True)
    build.add_argument("--checklist", default=str(DEFAULT_CHECKLIST_PATH))
    build.add_argument("--checklist-registry", default=str(DEFAULT_CHECKLIST_REGISTRY_PATH))
    build.add_argument("--issue-number", action="append", default=[])
    build.add_argument("--cleanup-status", required=True)
    build.add_argument("--cleanup-summary", required=True)
    build.add_argument("--generated-at")
    build.add_argument("--output")
    build.set_defaults(func=_cmd_build)

    validate = subparsers.add_parser("validate")
    validate.add_argument("--manifest", required=True)
    validate.add_argument("--now")
    validate.add_argument("--max-age-seconds", type=int, default=3600)
    validate.add_argument("--checklist", default=str(DEFAULT_CHECKLIST_PATH))
    validate.add_argument("--checklist-registry", default=str(DEFAULT_CHECKLIST_REGISTRY_PATH))
    validate.set_defaults(func=_cmd_validate)

    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
