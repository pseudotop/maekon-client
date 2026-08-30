#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
from pathlib import Path
import unittest


SCRIPT_PATH = Path(__file__).with_name("macos-bundle-version.py")
SPEC = importlib.util.spec_from_file_location("macos_bundle_version", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class AppleBundleVersionTest(unittest.TestCase):
    def test_stable_version_remains_numeric(self) -> None:
        self.assertEqual(MODULE.apple_bundle_version("1.23.45"), "1.23.45")

    def test_release_candidate_uses_apple_final_candidate_suffix(self) -> None:
        self.assertEqual(MODULE.apple_bundle_version("0.0.1-rc.10"), "0.0.1fc10")

    def test_alpha_and_beta_suffixes_are_supported(self) -> None:
        self.assertEqual(MODULE.apple_bundle_version("2.3.4-alpha.5"), "2.3.4a5")
        self.assertEqual(MODULE.apple_bundle_version("2.3.4-beta.6"), "2.3.4b6")

    def test_unsupported_or_out_of_range_versions_fail_closed(self) -> None:
        for version in ("0.0.1-rc.0", "0.0.1-rc.256", "0.0.1-preview.1", "1.2"):
            with self.subTest(version=version):
                with self.assertRaises(ValueError):
                    MODULE.apple_bundle_version(version)


if __name__ == "__main__":
    unittest.main()
