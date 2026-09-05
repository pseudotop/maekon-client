#!/usr/bin/env python3
"""Exercise the protected keypair gate without publishing release artifacts."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path


PRIVATE = "UPDATE_SIGNING_PRIVATE_KEY_B64"
PUBLIC = "MAEKON_UPDATE_PUBLIC_KEY"
ROOT = Path(__file__).resolve().parents[1]


def rehearse() -> dict:
    """Require the real signing probe and both negative controls to work."""
    public = os.environ.get(PUBLIC, "").strip()
    try:
        raw = base64.b64decode(public, validate=True)
    except ValueError:
        raise RuntimeError("Configured public key is invalid") from None
    if len(raw) != 32 or base64.b64encode(raw).decode("ascii") != public:
        raise RuntimeError("Configured public key is invalid")
    source = (ROOT / "src-tauri/src/updater/trusted_keys.rs").read_text()
    declaration = re.search(r"const TRUSTED_PUBLIC_KEYS:.*?=\s*&\[(.*?)\];", source, re.S)
    if declaration is None:
        raise RuntimeError("Trusted public key declaration is missing")
    entries = re.findall(r'^\s*"([A-Za-z0-9+/]{43}=)"\s*,', declaration[1], re.M)
    if public not in entries:
        raise RuntimeError("Configured public key is not trusted by this source")

    configured = dict(os.environ)
    missing = {name: value for name, value in configured.items() if name != PRIVATE}
    mismatch = dict(configured)
    mismatch[PUBLIC] = base64.b64encode(bytes([raw[0] ^ 1]) + raw[1:]).decode("ascii")
    cases = (
        ("configured_keypair", configured, 0, "updater signing keypair preflight passed\n", ""),
        ("missing_private_rejected", missing, 1, "", f"Missing required secret: {PRIVATE}\n"),
        ("mismatched_public_rejected", mismatch, 1, "", f"{PRIVATE} does not match {PUBLIC}\n"),
    )
    results = {}
    for name, environment, code, stdout, stderr in cases:
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts/verify-update-signing-keypair.py")],
            env=environment, capture_output=True, text=True, check=False, timeout=30,
        )
        # Never forward child output: even a broken verifier must not leak secrets.
        if (result.returncode, result.stdout, result.stderr) != (code, stdout, stderr):
            raise RuntimeError(f"Signing rehearsal failed: {name}")
        results[name] = "passed"
    return {
        "schema": "maekon.update-signing-rehearsal.v1",
        "public_key_sha256": hashlib.sha256(raw).hexdigest(),
        "checks": results,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    # Do not leave a stale success receipt if a later invocation fails.
    args.receipt.unlink(missing_ok=True)
    try:
        receipt = rehearse()
    except RuntimeError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    except (OSError, subprocess.SubprocessError, UnicodeError):
        print("Signing rehearsal could not complete", file=sys.stderr)
        return 1
    args.receipt.write_text(json.dumps(receipt, indent=2) + "\n")
    print("Signing keypair rehearsal passed; sanitized receipt written")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
