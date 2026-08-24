#!/usr/bin/env python3
from __future__ import annotations

import base64
import os
import subprocess
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-update-signing-keypair.py")
SEED = bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
PUBLIC_KEY = bytes.fromhex(
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
)
SEED_B64 = base64.b64encode(SEED).decode("ascii")
PUBLIC_KEY_B64 = base64.b64encode(PUBLIC_KEY).decode("ascii")


class UpdateSigningKeypairPreflightTests(unittest.TestCase):
    def _run(self, *, seed: str = SEED_B64, public_key: str = PUBLIC_KEY_B64):
        env = {
            **os.environ,
            "UPDATE_SIGNING_PRIVATE_KEY_B64": seed,
            "MAEKON_UPDATE_PUBLIC_KEY": public_key,
        }
        return subprocess.run(
            [sys.executable, str(SCRIPT)],
            check=False,
            capture_output=True,
            text=True,
            env=env,
        )

    def test_matching_rfc8032_keypair_passes_without_disclosure(self) -> None:
        completed = self._run()

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(
            completed.stdout.strip(),
            "updater signing keypair preflight passed",
        )
        output = completed.stdout + completed.stderr
        self.assertNotIn(SEED_B64, output)
        self.assertNotIn(PUBLIC_KEY_B64, output)

    def test_missing_private_key_fails_closed(self) -> None:
        completed = self._run(seed="")

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("Missing required secret", completed.stderr)

    def test_mismatched_public_key_fails_without_disclosure(self) -> None:
        other_public = base64.b64encode(bytes([PUBLIC_KEY[0] ^ 1]) + PUBLIC_KEY[1:])
        completed = self._run(public_key=other_public.decode("ascii"))

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("does not match", completed.stderr)
        output = completed.stdout + completed.stderr
        self.assertNotIn(SEED_B64, output)
        self.assertNotIn(PUBLIC_KEY_B64, output)

    def test_noncanonical_base64_fails_closed(self) -> None:
        completed = self._run(seed="not-base64")

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("canonical base64", completed.stderr)

    def test_noncanonical_pad_bits_fail_closed(self) -> None:
        alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        final_data_index = len(SEED_B64) - 2
        canonical_index = alphabet.index(SEED_B64[final_data_index])
        self.assertEqual(canonical_index & 0b11, 0)
        noncanonical = (
            SEED_B64[:final_data_index]
            + alphabet[canonical_index | 0b01]
            + SEED_B64[final_data_index + 1 :]
        )
        self.assertEqual(base64.b64decode(noncanonical), SEED)

        completed = self._run(seed=noncanonical)

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("canonical base64", completed.stderr)

    def test_wrong_key_length_fails_closed(self) -> None:
        completed = self._run(seed=base64.b64encode(b"short").decode("ascii"))

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("exactly 32 bytes", completed.stderr)


if __name__ == "__main__":
    unittest.main()
