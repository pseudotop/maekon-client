#!/usr/bin/env python3
"""Fail closed when the protected updater signing keypair is not ready."""

from __future__ import annotations

import base64
import binascii
import hmac
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


PRIVATE_KEY_ENV = "UPDATE_SIGNING_PRIVATE_KEY_B64"
PUBLIC_KEY_ENV = "MAEKON_UPDATE_PUBLIC_KEY"
PRIVATE_DER_PREFIX = bytes.fromhex("302e020100300506032b657004220420")
PUBLIC_DER_PREFIX = bytes.fromhex("302a300506032b6570032100")
PROBE = b"maekon-update-signing-keypair-preflight-v1\n"


def _decode_key(name: str) -> bytes:
    value = os.environ.get(name, "").strip()
    if not value:
        raise SystemExit(f"Missing required secret: {name}")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (binascii.Error, ValueError) as exc:
        raise SystemExit(f"{name} must be canonical base64") from exc
    if base64.b64encode(decoded).decode("ascii") != value:
        raise SystemExit(f"{name} must be canonical base64")
    if len(decoded) != 32:
        raise SystemExit(f"{name} must decode to exactly 32 bytes")
    return decoded


def _run_openssl(*args: str) -> None:
    completed = subprocess.run(
        ["openssl", *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise SystemExit("OpenSSL rejected the updater signing keypair preflight")


def verify_keypair() -> None:
    if shutil.which("openssl") is None:
        raise SystemExit("OpenSSL is required for updater signing keypair preflight")

    seed = _decode_key(PRIVATE_KEY_ENV)
    expected_public_key = _decode_key(PUBLIC_KEY_ENV)

    with tempfile.TemporaryDirectory(prefix="maekon-signing-preflight-") as temp_dir:
        root = Path(temp_dir)
        private_der = root / "private.der"
        derived_public_der = root / "derived-public.der"
        expected_public_der = root / "expected-public.der"
        probe = root / "probe.bin"
        signature = root / "probe.sig"

        private_der.write_bytes(PRIVATE_DER_PREFIX + seed)
        private_der.chmod(0o600)
        expected_public_der.write_bytes(PUBLIC_DER_PREFIX + expected_public_key)
        expected_public_der.chmod(0o600)
        probe.write_bytes(PROBE)

        _run_openssl(
            "pkey",
            "-in",
            str(private_der),
            "-inform",
            "DER",
            "-pubout",
            "-outform",
            "DER",
            "-out",
            str(derived_public_der),
        )
        derived = derived_public_der.read_bytes()
        expected = expected_public_der.read_bytes()
        if not hmac.compare_digest(derived, expected):
            raise SystemExit(f"{PRIVATE_KEY_ENV} does not match {PUBLIC_KEY_ENV}")

        _run_openssl(
            "pkeyutl",
            "-sign",
            "-rawin",
            "-inkey",
            str(private_der),
            "-keyform",
            "DER",
            "-in",
            str(probe),
            "-out",
            str(signature),
        )
        _run_openssl(
            "pkeyutl",
            "-verify",
            "-rawin",
            "-pubin",
            "-inkey",
            str(expected_public_der),
            "-keyform",
            "DER",
            "-in",
            str(probe),
            "-sigfile",
            str(signature),
        )


def main() -> int:
    verify_keypair()
    print("updater signing keypair preflight passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
