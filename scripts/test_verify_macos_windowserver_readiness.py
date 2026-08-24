#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock


SCRIPT = Path(__file__).with_name("verify-macos-windowserver-readiness.py")


def _load_module():
    spec = importlib.util.spec_from_file_location(
        "maekon_windowserver_readiness", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


readiness = _load_module()


def _snapshot(**overrides):
    values = {
        "console_user": "maekon-smoke",
        "current_user": "maekon-smoke",
        "gui_session_available": True,
        "hardware_model": "Mac14,3",
        "platform_name": "Darwin",
        "screen_locked": False,
        "windowserver_running": True,
    }
    values.update(overrides)
    return readiness.ProbeSnapshot(**values)


class MacOSWindowServerReadinessTests(unittest.TestCase):
    def _write_runner_config(self, directory: str, **overrides) -> Path:
        payload = {
            "schema_version": 1,
            "purpose": "maekon-desktop-smoke",
            "dedicated": True,
            "development_host": False,
            "disposable_user": "maekon-smoke",
            "runner_scope": "repository",
            "tcc_mutation_policy": "forbidden",
        }
        payload.update(overrides)
        path = Path(directory) / "runner.json"
        path.write_text(json.dumps(payload), encoding="utf-8")
        return path

    def test_ready_positive_control_passes(self) -> None:
        verdict = readiness.evaluate(_snapshot(), expected_state="ready")

        self.assertTrue(verdict.observed_as_expected)
        self.assertEqual(verdict.observed_state, "ready")
        self.assertEqual(verdict.reason_codes, ())

    def test_locked_negative_control_passes_only_as_unavailable(self) -> None:
        snapshot = _snapshot(screen_locked=True)

        negative = readiness.evaluate(snapshot, expected_state="unavailable")
        positive = readiness.evaluate(snapshot, expected_state="ready")

        self.assertTrue(negative.observed_as_expected)
        self.assertFalse(positive.observed_as_expected)
        self.assertIn("screen_locked", negative.reason_codes)

    def test_available_session_cannot_false_pass_negative_control(self) -> None:
        verdict = readiness.evaluate(_snapshot(), expected_state="unavailable")

        self.assertFalse(verdict.observed_as_expected)
        self.assertEqual(verdict.observed_state, "ready")

    def test_virtualized_hardware_fails_both_controls(self) -> None:
        snapshot = _snapshot(hardware_model="VirtualMac2,1", screen_locked=True)

        for expected in ("ready", "unavailable"):
            verdict = readiness.evaluate(snapshot, expected_state=expected)
            self.assertFalse(verdict.observed_as_expected)
            self.assertIn("virtualized_hardware", verdict.reason_codes)

    def test_lock_parser_accepts_known_ioreg_boolean_shapes(self) -> None:
        self.assertTrue(readiness._screen_is_locked('"CGSSessionScreenIsLocked" = Yes'))
        self.assertTrue(readiness._screen_is_locked('"IOConsoleLocked" = <true/>'))
        self.assertFalse(readiness._screen_is_locked('"CGSSessionScreenIsLocked" = No'))

    def test_receipt_excludes_raw_host_identifiers(self) -> None:
        snapshot = _snapshot(
            console_user="private-runner-user",
            current_user="private-runner-user",
            hardware_model="Mac14,3-private-serial",
        )
        verdict = readiness.evaluate(snapshot, expected_state="ready")
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "receipt.json"
            readiness._write_receipt(output, verdict)
            rendered = output.read_text(encoding="utf-8")
            payload = json.loads(rendered)

        self.assertTrue(payload["observed_as_expected"])
        self.assertNotIn(snapshot.current_user, rendered)
        self.assertNotIn(snapshot.hardware_model, rendered)

    def test_root_owned_runner_contract_accepts_disposable_user(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = self._write_runner_config(directory)
            root_owned = SimpleNamespace(st_uid=0, st_mode=0o100644)
            with mock.patch.object(Path, "stat", return_value=root_owned):
                config = readiness.load_config(path, current_user="maekon-smoke")

        self.assertTrue(config.dedicated)
        self.assertEqual(config.runner_scope, "repository")

    def test_development_host_declaration_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = self._write_runner_config(directory, development_host=True)
            root_owned = SimpleNamespace(st_uid=0, st_mode=0o100644)
            with mock.patch.object(Path, "stat", return_value=root_owned):
                with self.assertRaisesRegex(
                    readiness.ReadinessError, "development hosts"
                ):
                    readiness.load_config(path, current_user="maekon-smoke")


if __name__ == "__main__":
    unittest.main()
