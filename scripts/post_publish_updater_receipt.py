#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA_VERSION = "maekon.post_publish_updater_receipt.v1"
RC_TAG_PATTERN = re.compile(r"^v(?P<base>\d+\.\d+\.\d+)-rc\.(?P<number>\d+)$")
SAFE_STATUSES = {"safe", "redacted"}


def _load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def _parse_utc(value: str) -> datetime:
    if not value.endswith("Z"):
        raise ValueError(f"timestamp must be UTC/Z: {value}")
    return datetime.fromisoformat(value.removesuffix("Z") + "+00:00").astimezone(timezone.utc)


def _is_git_sha(value: Any) -> bool:
    return isinstance(value, str) and bool(re.fullmatch(r"[0-9a-f]{40}", value))


def _is_sha256(value: Any) -> bool:
    return isinstance(value, str) and bool(re.fullmatch(r"[0-9a-f]{64}", value))


def validate_receipt(
    receipt: Any,
    *,
    release_tag: str,
    commit_sha: str,
    previous_commit_sha: str,
    now: str | None = None,
    max_age_seconds: int = 86_400,
) -> list[str]:
    errors: list[str] = []
    if not isinstance(receipt, dict):
        return ["receipt must be an object"]
    if max_age_seconds <= 0:
        errors.append("max_age_seconds must be positive")
    if receipt.get("schema_version") != SCHEMA_VERSION:
        errors.append(f"receipt must use {SCHEMA_VERSION}")

    expected_match = RC_TAG_PATTERN.fullmatch(release_tag)
    if expected_match is None:
        errors.append("expected release tag must use vX.Y.Z-rc.N")
    if not _is_git_sha(commit_sha):
        errors.append("expected commit SHA must contain 40 lowercase hex characters")
    if not _is_git_sha(previous_commit_sha):
        errors.append("expected previous commit SHA must contain 40 lowercase hex characters")
    if receipt.get("release_tag") != release_tag:
        errors.append("receipt release_tag does not match the requested tag")
    if receipt.get("release_commit_sha") != commit_sha:
        errors.append("receipt release_commit_sha does not match the tag commit")
    if receipt.get("previous_release_commit_sha") != previous_commit_sha:
        errors.append("receipt previous_release_commit_sha does not match the previous tag commit")
    if receipt.get("detected_tag") != release_tag:
        errors.append("detected_tag does not match the requested tag")

    previous_tag = receipt.get("previous_release_tag")
    previous_match = RC_TAG_PATTERN.fullmatch(previous_tag) if isinstance(previous_tag, str) else None
    if previous_match is None:
        errors.append("previous_release_tag must use vX.Y.Z-rc.N")
    elif expected_match is not None:
        if previous_match.group("base") != expected_match.group("base"):
            errors.append("previous release and requested release must share the same base version")
        if int(previous_match.group("number")) >= int(expected_match.group("number")):
            errors.append("previous release candidate must precede the requested release candidate")

    if receipt.get("channel") != "prerelease":
        errors.append("receipt channel must be prerelease")
    if receipt.get("result") != "available":
        errors.append("receipt result must be available")
    if receipt.get("detection_source") != "previous-rc-runtime":
        errors.append("detection_source must be previous-rc-runtime")
    observer = receipt.get("observer")
    if not isinstance(observer, str) or not observer.strip():
        errors.append("observer is required")

    observed_at = receipt.get("observed_at")
    if not isinstance(observed_at, str):
        errors.append("observed_at is required")
    else:
        try:
            observed = _parse_utc(observed_at)
            current = _parse_utc(now) if now else datetime.now(timezone.utc)
            age_seconds = (current - observed).total_seconds()
            if age_seconds < 0:
                errors.append("observed_at cannot be in the future")
            if age_seconds > max_age_seconds:
                errors.append("post-publish updater receipt is stale")
        except ValueError as exc:
            errors.append(str(exc))

    evidence = receipt.get("evidence")
    if not isinstance(evidence, dict):
        errors.append("evidence is required")
    else:
        uri = evidence.get("uri")
        if not isinstance(uri, str) or not re.fullmatch(r"(?:artifact|evidence)://.+", uri):
            errors.append("evidence uri must use artifact:// or evidence://")
        if not _is_sha256(evidence.get("sha256")):
            errors.append("evidence sha256 must contain 64 lowercase hex characters")
        if evidence.get("privacy_status") not in SAFE_STATUSES:
            errors.append("evidence privacy status is not shareable")
        if evidence.get("redaction_status") not in SAFE_STATUSES:
            errors.append("evidence redaction status is not shareable")
        if evidence.get("sanitized") is not True:
            errors.append("evidence must be sanitized")

    return errors


def _cmd_validate(args: argparse.Namespace) -> int:
    try:
        receipt = _load_json(Path(args.receipt))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"post-publish updater receipt rejection: {exc}", file=sys.stderr)
        return 1

    errors = validate_receipt(
        receipt,
        release_tag=args.release_tag,
        commit_sha=args.commit_sha,
        previous_commit_sha=args.previous_commit_sha,
        now=args.now,
        max_age_seconds=args.max_age_seconds,
    )
    if errors:
        for error in errors:
            print(f"post-publish updater receipt rejection: {error}", file=sys.stderr)
        return 1
    print("post-publish updater receipt accepted")
    return 0


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Validate a previous-RC observation of a newly published Maekon RC."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate = subparsers.add_parser("validate")
    validate.add_argument("--receipt", required=True)
    validate.add_argument("--release-tag", required=True)
    validate.add_argument("--commit-sha", required=True)
    validate.add_argument("--previous-commit-sha", required=True)
    validate.add_argument("--now")
    validate.add_argument("--max-age-seconds", type=int, default=86_400)
    validate.set_defaults(func=_cmd_validate)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
