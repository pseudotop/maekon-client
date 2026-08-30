#!/usr/bin/env python3
"""Merge and verify canonical macOS TCC usage descriptions."""

from __future__ import annotations

import argparse
import os
import plistlib
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any


REQUIRED_TCC_KEYS = (
    "NSScreenCaptureUsageDescription",
    "NSAccessibilityUsageDescription",
    "NSMicrophoneUsageDescription",
    "NSInputMonitoringUsageDescription",
)


class TCCUsageDescriptionError(ValueError):
    """Raised when a plist violates the TCC usage-description contract."""


def _load_plist(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as plist_file:
            value = plistlib.load(plist_file)
    except (OSError, plistlib.InvalidFileException) as exc:
        raise TCCUsageDescriptionError(f"could not read plist {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise TCCUsageDescriptionError(f"plist root must be a dictionary: {path}")
    return value


def canonical_descriptions(canonical_path: Path) -> dict[str, str]:
    canonical = _load_plist(canonical_path)
    descriptions: dict[str, str] = {}
    for key in REQUIRED_TCC_KEYS:
        value = canonical.get(key)
        if not isinstance(value, str) or not value.strip():
            raise TCCUsageDescriptionError(
                f"canonical plist must contain a non-empty string for {key}: "
                f"{canonical_path}"
            )
        descriptions[key] = value
    return descriptions


def merge_descriptions(canonical_path: Path, target_path: Path) -> None:
    descriptions = canonical_descriptions(canonical_path)
    target = _load_plist(target_path)
    target_mode = stat.S_IMODE(target_path.stat().st_mode)
    target.update(descriptions)

    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", dir=target_path.parent, prefix=f".{target_path.name}.", delete=False
        ) as temporary_file:
            temporary_path = Path(temporary_file.name)
            plistlib.dump(target, temporary_file, fmt=plistlib.FMT_XML, sort_keys=True)
        temporary_path.chmod(target_mode)
        os.replace(temporary_path, target_path)
    except OSError as exc:
        raise TCCUsageDescriptionError(f"could not write plist {target_path}: {exc}") from exc
    finally:
        if "temporary_path" in locals():
            temporary_path.unlink(missing_ok=True)


def verify_descriptions(canonical_path: Path, target_path: Path) -> None:
    descriptions = canonical_descriptions(canonical_path)
    target = _load_plist(target_path)
    for key, expected in descriptions.items():
        actual = target.get(key)
        if not isinstance(actual, str) or not actual.strip():
            raise TCCUsageDescriptionError(
                f"target plist must contain a non-empty string for {key}: {target_path}"
            )
        if actual != expected:
            raise TCCUsageDescriptionError(
                f"target plist value for {key} does not match canonical source: {target_path}"
            )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Merge or verify canonical macOS TCC usage descriptions."
    )
    parser.add_argument("command", choices=("merge", "verify"))
    parser.add_argument("--canonical", required=True, type=Path)
    parser.add_argument("--target", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.command == "merge":
            merge_descriptions(args.canonical, args.target)
        verify_descriptions(args.canonical, args.target)
    except TCCUsageDescriptionError as exc:
        print(f"macOS TCC usage-description error: {exc}", file=sys.stderr)
        return 1

    print(
        f"macOS TCC usage descriptions {args.command} verified: {args.target} "
        f"({len(REQUIRED_TCC_KEYS)} keys)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
