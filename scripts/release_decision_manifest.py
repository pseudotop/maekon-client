#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA_VERSION = "automation.gui.benchmark_report.v1.release_decision"
SOURCE_REPORT_SCHEMA_VERSION = "automation.gui.benchmark_report.v1"
EVIDENCE_POLICY_SCHEMA_VERSION = "automation.gui.permission_evidence.v1"

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


def _load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


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


def _manifest_blockers(manifest: dict[str, Any], *, now: str | None = None, max_age_seconds: int = 3600) -> list[str]:
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
    covered_issue_numbers: list[int],
    cleanup_status: str,
    cleanup_summary: str,
    generated_at: str | None = None,
) -> dict[str, Any]:
    report = _load_json(benchmark_report_path)
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
        "release_decision": {
            "state": "pass",
            "reasons": [],
        },
    }
    blockers = _manifest_blockers(manifest, now=generated_at)
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
) -> list[str]:
    return _manifest_blockers(manifest, now=now, max_age_seconds=max_age_seconds)


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
        covered_issue_numbers=[int(issue) for issue in args.issue_number],
        cleanup_status=args.cleanup_status,
        cleanup_summary=args.cleanup_summary,
        generated_at=args.generated_at,
    )
    output = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    if args.output:
        Path(args.output).write_text(output, encoding="utf-8")
    else:
        print(output, end="")
    return 0 if manifest["release_decision"]["state"] == "pass" else 1


def _cmd_validate(args: argparse.Namespace) -> int:
    manifest = _load_json(Path(args.manifest))
    errors = validate_manifest(
        manifest,
        now=args.now,
        max_age_seconds=args.max_age_seconds,
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
    build.add_argument("--evidence-artifacts")
    build.add_argument("--claims")
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
    validate.set_defaults(func=_cmd_validate)

    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
