#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("sanitize_desktop_evidence.py")


def _load_module():
    spec = importlib.util.spec_from_file_location("maekon_desktop_sanitizer", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError("unable to load desktop evidence sanitizer")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


sanitizer = _load_module()


def _readiness(*, expected: str, observed: str, reasons: list[str]) -> str:
    return json.dumps(
        {
            "schema_version": 1,
            "probe": "maekon-macos-windowserver-readiness",
            "dedicated_config": "pass",
            "hardware_isolation": "pass",
            "expected_state": expected,
            "observed_state": observed,
            "observed_as_expected": True,
            "reason_codes": reasons,
        },
        sort_keys=True,
    )


def _cleanup() -> str:
    return json.dumps(
        {
            "schema_version": 1,
            "cleanup_status": "pass",
            "profile_absent": True,
            "process_absent": True,
            "tcc_mutation": "not_performed",
        },
        sort_keys=True,
    )


def _items(readiness: str, cleanup: str) -> list[dict[str, str]]:
    return [
        {
            "id": "windowserver-gui-smoke-log",
            "path": "runtime.log",
            "artifact_kind": "log_excerpt",
            "content": "",
        },
        {
            "id": "windowserver-readiness-receipt",
            "path": "readiness.json",
            "artifact_kind": "log_excerpt",
            "content": readiness,
        },
        {
            "id": "windowserver-cleanup-receipt",
            "path": "cleanup.json",
            "artifact_kind": "log_excerpt",
            "content": cleanup,
        },
    ]


class WindowServerStructuredEvidenceTests(unittest.TestCase):
    def _sanitize(self, readiness: str, cleanup: str):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        output_dir = Path(directory.name)
        bundle = sanitizer.sanitize_bundle(
            inputs=_items(readiness, cleanup),
            output_dir=output_dir,
            commit_sha="a" * 40,
            release_tag="v0.0.1-rc.10",
            artifact_checksum="sha256:" + "b" * 64,
            runner_label="macos-windowserver",
            cleanup_status="pass",
            generated_at="2026-08-25T00:00:00Z",
        )
        return bundle, output_dir

    def test_ready_control_retains_allowlisted_machine_readable_fields(self) -> None:
        bundle, _ = self._sanitize(_readiness(expected="ready", observed="ready", reasons=[]), _cleanup())

        self.assertEqual(bundle["release_decision_state"], "optional")
        artifacts = {item["id"]: item for item in bundle["artifacts"]}
        evidence = artifacts["windowserver-readiness-receipt"]["structured_evidence"]
        self.assertEqual(evidence["expected_state"], "ready")
        self.assertEqual(evidence["observed_state"], "ready")
        self.assertTrue(evidence["observed_as_expected"])
        self.assertEqual(evidence["reason_codes"], [])

    def test_locked_negative_and_cleanup_receipts_remain_actionable(self) -> None:
        bundle, _ = self._sanitize(
            _readiness(
                expected="unavailable",
                observed="unavailable",
                reasons=["screen_locked"],
            ),
            _cleanup(),
        )

        artifacts = {item["id"]: item for item in bundle["artifacts"]}
        readiness = artifacts["windowserver-readiness-receipt"]["structured_evidence"]
        cleanup = artifacts["windowserver-cleanup-receipt"]["structured_evidence"]
        self.assertEqual(readiness["reason_codes"], ["screen_locked"])
        self.assertEqual(cleanup["cleanup_status"], "pass")
        self.assertTrue(cleanup["profile_absent"])
        self.assertTrue(cleanup["process_absent"])
        self.assertEqual(cleanup["tcc_mutation"], "not_performed")

    def test_missing_receipt_blocks_and_cli_exits_nonzero(self) -> None:
        bundle, _ = self._sanitize("", _cleanup())
        self.assertEqual(bundle["release_decision_state"], "blocked_for_privacy")
        self.assertIn("windowserver-readiness-receipt:missing_receipt", bundle["errors"])

        cleanup_missing, _ = self._sanitize(
            _readiness(expected="ready", observed="ready", reasons=[]), ""
        )
        self.assertEqual(
            cleanup_missing["release_decision_state"], "blocked_for_privacy"
        )
        self.assertIn(
            "windowserver-cleanup-receipt:missing_receipt",
            cleanup_missing["errors"],
        )

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cleanup_path = root / "cleanup.json"
            cleanup_path.write_text(_cleanup(), encoding="utf-8")
            completed = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--input",
                    str(root / "missing-readiness.json"),
                    "--input-id",
                    "windowserver-readiness-receipt",
                    "--input",
                    str(cleanup_path),
                    "--input-id",
                    "windowserver-cleanup-receipt",
                    "--output-dir",
                    str(root / "output"),
                    "--commit-sha",
                    "a" * 40,
                    "--release-tag",
                    "v0.0.1-rc.10",
                    "--artifact-checksum",
                    "sha256:" + "b" * 64,
                    "--runner-label",
                    "macos-windowserver",
                    "--cleanup-status",
                    "pass",
                ],
                check=False,
                capture_output=True,
                text=True,
            )
        self.assertEqual(completed.returncode, 1)

        with tempfile.TemporaryDirectory() as directory:
            output_dir = Path(directory)
            absent_cleanup = sanitizer.sanitize_bundle(
                inputs=_items(
                    _readiness(expected="ready", observed="ready", reasons=[]),
                    _cleanup(),
                )[:-1],
                output_dir=output_dir,
                commit_sha="a" * 40,
                release_tag="v0.0.1-rc.10",
                artifact_checksum="sha256:" + "b" * 64,
                runner_label="macos-windowserver",
                cleanup_status="pass",
                generated_at="2026-08-25T00:00:00Z",
            )
        self.assertEqual(absent_cleanup["release_decision_state"], "blocked_for_privacy")
        self.assertIn(
            "windowserver-cleanup-receipt:missing_receipt",
            absent_cleanup["errors"],
        )

    def test_extra_raw_identity_field_is_rejected_without_disclosure(self) -> None:
        private_user = "private-runner-user"
        payload = json.loads(_readiness(expected="ready", observed="ready", reasons=[]))
        payload["console_user"] = private_user
        bundle, output_dir = self._sanitize(json.dumps(payload), _cleanup())

        self.assertEqual(bundle["release_decision_state"], "blocked_for_privacy")
        self.assertIn(
            "windowserver-readiness-receipt:fields_do_not_match_contract",
            bundle["errors"],
        )
        rendered = "\n".join(
            path.read_text(encoding="utf-8") for path in output_dir.glob("*.json")
        )
        self.assertNotIn(private_user, rendered)

    def test_contradictory_control_is_rejected(self) -> None:
        bundle, _ = self._sanitize(
            _readiness(
                expected="ready",
                observed="unavailable",
                reasons=["screen_locked"],
            ),
            _cleanup(),
        )
        self.assertEqual(bundle["release_decision_state"], "blocked_for_privacy")
        self.assertIn(
            "windowserver-readiness-receipt:ready_control_is_contradictory",
            bundle["errors"],
        )

        identity_failure, _ = self._sanitize(
            _readiness(
                expected="unavailable",
                observed="unavailable",
                reasons=["screen_locked", "virtualized_hardware"],
            ),
            _cleanup(),
        )
        self.assertEqual(
            identity_failure["release_decision_state"], "blocked_for_privacy"
        )
        self.assertIn(
            "windowserver-readiness-receipt:hardware_isolation_is_contradictory",
            identity_failure["errors"],
        )


if __name__ == "__main__":
    unittest.main()
