#!/usr/bin/env python3
"""Tests for the macOS TCC usage-description release contract."""

from __future__ import annotations

import plistlib
import tempfile
import unittest
from pathlib import Path

from macos_tcc_usage_descriptions import (
    REQUIRED_TCC_KEYS,
    TCCUsageDescriptionError,
    merge_descriptions,
    verify_descriptions,
)


class TCCUsageDescriptionsTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary_directory.name)
        self.canonical = self.root / "canonical.plist"
        self.target = self.root / "target.plist"
        self.descriptions = {
            key: f"Canonical permission description for {key}."
            for key in REQUIRED_TCC_KEYS
        }
        self._write(self.canonical, self.descriptions)
        self._write(
            self.target,
            {
                "CFBundleIdentifier": "com.maekon.app",
                "UnrelatedNestedValue": {"preserved": True},
            },
        )

    def tearDown(self) -> None:
        self.temporary_directory.cleanup()

    @staticmethod
    def _write(path: Path, value: object) -> None:
        with path.open("wb") as plist_file:
            plistlib.dump(value, plist_file)

    @staticmethod
    def _read(path: Path) -> dict[str, object]:
        with path.open("rb") as plist_file:
            value = plistlib.load(plist_file)
        assert isinstance(value, dict)
        return value

    def test_merge_copies_exact_values_and_preserves_unrelated_entries(self) -> None:
        self.target.chmod(0o644)
        merge_descriptions(self.canonical, self.target)

        merged = self._read(self.target)
        for key, expected in self.descriptions.items():
            self.assertEqual(merged[key], expected)
        self.assertEqual(merged["CFBundleIdentifier"], "com.maekon.app")
        self.assertEqual(merged["UnrelatedNestedValue"], {"preserved": True})
        first_merge = self.target.read_bytes()
        merge_descriptions(self.canonical, self.target)
        self.assertEqual(self.target.read_bytes(), first_merge)
        self.assertEqual(self.target.stat().st_mode & 0o777, 0o644)
        verify_descriptions(self.canonical, self.target)

    def test_deleting_each_required_key_fails_verification(self) -> None:
        merge_descriptions(self.canonical, self.target)
        merged = self._read(self.target)

        for key in REQUIRED_TCC_KEYS:
            with self.subTest(key=key):
                mutated = dict(merged)
                del mutated[key]
                self._write(self.target, mutated)
                with self.assertRaisesRegex(TCCUsageDescriptionError, key):
                    verify_descriptions(self.canonical, self.target)

    def test_empty_and_noncanonical_values_fail_verification(self) -> None:
        merge_descriptions(self.canonical, self.target)
        merged = self._read(self.target)

        for value in ("", "Different copy"):
            with self.subTest(value=value):
                mutated = dict(merged)
                mutated[REQUIRED_TCC_KEYS[0]] = value
                self._write(self.target, mutated)
                with self.assertRaisesRegex(
                    TCCUsageDescriptionError, REQUIRED_TCC_KEYS[0]
                ):
                    verify_descriptions(self.canonical, self.target)

    def test_invalid_canonical_source_is_rejected(self) -> None:
        canonical = dict(self.descriptions)
        del canonical[REQUIRED_TCC_KEYS[-1]]
        self._write(self.canonical, canonical)

        with self.assertRaisesRegex(TCCUsageDescriptionError, REQUIRED_TCC_KEYS[-1]):
            merge_descriptions(self.canonical, self.target)


if __name__ == "__main__":
    unittest.main()
