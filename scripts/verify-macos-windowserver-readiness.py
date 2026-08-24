#!/usr/bin/env python3
"""Fail-closed readiness probe for the dedicated macOS desktop-smoke runner."""

from __future__ import annotations

import argparse
import json
import os
import platform
import pwd
import re
import stat
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Sequence


CONFIG_PATH = Path("/etc/maekon/desktop-smoke-runner.json")
UNAVAILABLE_REASONS = {
    "console_session_unavailable",
    "gui_session_unavailable",
    "screen_locked",
    "windowserver_unavailable",
}
VIRTUAL_HARDWARE_MARKERS = (
    "parallels",
    "qemu",
    "tart",
    "virtualbox",
    "virtualmac",
    "vmware",
)


class ReadinessError(RuntimeError):
    """Raised when the runner identity configuration is not trustworthy."""


@dataclass(frozen=True)
class RunnerConfig:
    disposable_user: str
    dedicated: bool
    development_host: bool
    purpose: str
    runner_scope: str
    schema_version: int
    tcc_mutation_policy: str


@dataclass(frozen=True)
class ProbeSnapshot:
    console_user: str
    current_user: str
    gui_session_available: bool
    hardware_model: str
    platform_name: str
    screen_locked: bool
    windowserver_running: bool


@dataclass(frozen=True)
class Verdict:
    expected_state: str
    observed_state: str
    observed_as_expected: bool
    reason_codes: tuple[str, ...]


def _run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        check=False,
        capture_output=True,
        text=True,
        timeout=10,
    )


def _read_required(command: Sequence[str]) -> str:
    completed = _run(command)
    if completed.returncode != 0:
        return ""
    return completed.stdout.strip()


def _screen_is_locked(ioreg_output: str) -> bool:
    lock_keys = r"(?:CGSSessionScreenIsLocked|IOConsoleLocked)"
    return bool(
        re.search(
            rf'"{lock_keys}"\s*=\s*(?:Yes|true|1|<true/>)',
            ioreg_output,
            flags=re.IGNORECASE,
        )
    )


def load_config(path: Path, *, current_user: str) -> RunnerConfig:
    try:
        metadata = path.stat()
    except OSError as exc:
        raise ReadinessError("dedicated runner configuration is missing") from exc

    if metadata.st_uid != 0:
        raise ReadinessError("dedicated runner configuration must be root-owned")
    if stat.S_IMODE(metadata.st_mode) & 0o022:
        raise ReadinessError(
            "dedicated runner configuration must not be group/world writable"
        )

    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ReadinessError("dedicated runner configuration is invalid") from exc
    if not isinstance(payload, dict):
        raise ReadinessError("dedicated runner configuration must be an object")

    required = {
        "schema_version",
        "purpose",
        "dedicated",
        "development_host",
        "disposable_user",
        "runner_scope",
        "tcc_mutation_policy",
    }
    if set(payload) != required:
        raise ReadinessError(
            "dedicated runner configuration fields do not match the contract"
        )

    config = RunnerConfig(**payload)
    if config.schema_version != 1:
        raise ReadinessError("unsupported dedicated runner configuration version")
    if config.purpose != "maekon-desktop-smoke" or config.dedicated is not True:
        raise ReadinessError("runner is not dedicated to Maekon desktop smoke")
    if config.development_host is not False:
        raise ReadinessError("development hosts are forbidden")
    if config.runner_scope != "repository":
        raise ReadinessError("runner registration must be repository-scoped")
    if config.tcc_mutation_policy != "forbidden":
        raise ReadinessError("runner must forbid workflow-driven TCC mutation")
    if config.disposable_user != current_user:
        raise ReadinessError("workflow user does not match the disposable runner user")
    return config


def collect_snapshot() -> ProbeSnapshot:
    current_user = pwd.getpwuid(os.getuid()).pw_name
    console_user = _read_required(["/usr/bin/stat", "-f", "%Su", "/dev/console"])
    hardware_model = _read_required(["/usr/sbin/sysctl", "-n", "hw.model"])
    windowserver_running = (
        _run(["/usr/bin/pgrep", "-x", "WindowServer"]).returncode == 0
    )
    ioreg_output = _read_required(["/usr/sbin/ioreg", "-n", "Root", "-d", "1"])
    gui_session_available = (
        _run(["/bin/launchctl", "print", f"gui/{os.getuid()}"]).returncode == 0
    )
    return ProbeSnapshot(
        console_user=console_user,
        current_user=current_user,
        gui_session_available=gui_session_available,
        hardware_model=hardware_model,
        platform_name=platform.system(),
        screen_locked=_screen_is_locked(ioreg_output),
        windowserver_running=windowserver_running,
    )


def evaluate(snapshot: ProbeSnapshot, *, expected_state: str) -> Verdict:
    identity_reasons: list[str] = []
    if snapshot.platform_name != "Darwin":
        identity_reasons.append("unsupported_platform")
    if not snapshot.hardware_model:
        identity_reasons.append("hardware_model_unavailable")
    elif any(
        marker in snapshot.hardware_model.casefold()
        for marker in VIRTUAL_HARDWARE_MARKERS
    ):
        identity_reasons.append("virtualized_hardware")

    availability_reasons: list[str] = []
    if (
        not snapshot.console_user
        or snapshot.console_user in {"loginwindow", "root"}
        or snapshot.console_user != snapshot.current_user
    ):
        availability_reasons.append("console_session_unavailable")
    if not snapshot.windowserver_running:
        availability_reasons.append("windowserver_unavailable")
    if snapshot.screen_locked:
        availability_reasons.append("screen_locked")
    if not snapshot.gui_session_available:
        availability_reasons.append("gui_session_unavailable")

    reasons = tuple(sorted(set(identity_reasons + availability_reasons)))
    observed_state = "ready" if not availability_reasons else "unavailable"
    if identity_reasons:
        observed_as_expected = False
    elif expected_state == "ready":
        observed_as_expected = observed_state == "ready"
    else:
        observed_as_expected = bool(set(availability_reasons) & UNAVAILABLE_REASONS)
    return Verdict(
        expected_state=expected_state,
        observed_state=observed_state,
        observed_as_expected=observed_as_expected,
        reason_codes=reasons,
    )


def _write_receipt(path: Path, verdict: Verdict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    hardware_failures = {
        "unsupported_platform",
        "hardware_model_unavailable",
        "virtualized_hardware",
    }
    payload = {
        "schema_version": 1,
        "probe": "maekon-macos-windowserver-readiness",
        "dedicated_config": "pass",
        "hardware_isolation": "fail"
        if hardware_failures & set(verdict.reason_codes)
        else "pass",
        **asdict(verdict),
    }
    path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--expected-state", choices=("ready", "unavailable"), required=True
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--config", type=Path, default=CONFIG_PATH)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    snapshot = collect_snapshot()
    try:
        load_config(args.config, current_user=snapshot.current_user)
    except ReadinessError as exc:
        print(f"macOS WindowServer readiness failed closed: {exc}", file=sys.stderr)
        return 2

    verdict = evaluate(snapshot, expected_state=args.expected_state)
    _write_receipt(args.output, verdict)
    if not verdict.observed_as_expected:
        print(
            "macOS WindowServer readiness did not match the requested control",
            file=sys.stderr,
        )
        return 1
    print(f"macOS WindowServer readiness matched expected state: {args.expected_state}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
