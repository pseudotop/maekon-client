#!/usr/bin/env python3
"""Convert a supported Maekon SemVer into an Apple CFBundleVersion."""

from __future__ import annotations

import re
import sys


SEMVER_RE = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-(alpha|beta|rc)\.(0|[1-9][0-9]*))?$"
)
APPLE_SUFFIX = {"alpha": "a", "beta": "b", "rc": "fc"}


def apple_bundle_version(version: str) -> str:
    match = SEMVER_RE.fullmatch(version)
    if match is None:
        raise ValueError(
            "expected MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-(alpha|beta|rc).N"
        )

    major, minor, patch, prerelease, prerelease_number = match.groups()
    if len(major) > 4 or len(minor) > 2 or len(patch) > 2:
        raise ValueError(
            "Apple CFBundleVersion allows 4 digits for major and 2 digits each "
            "for minor and patch"
        )

    base = f"{major}.{minor}.{patch}"
    if prerelease is None:
        return base

    assert prerelease_number is not None
    number = int(prerelease_number)
    if not 1 <= number <= 255:
        raise ValueError("Apple prerelease build suffix must be between 1 and 255")
    return f"{base}{APPLE_SUFFIX[prerelease]}{number}"


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {argv[0]} <semver>", file=sys.stderr)
        return 2

    try:
        print(apple_bundle_version(argv[1]))
    except ValueError as exc:
        print(f"error: unsupported macOS bundle version {argv[1]!r}: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
