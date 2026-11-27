#!/usr/bin/env python3
from __future__ import annotations

import base64
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-release-final-bytes.py")
PRIVATE_DER_PREFIX = bytes.fromhex("302e020100300506032b657004220420")
PUBLIC_DER_PREFIX = bytes.fromhex("302a300506032b6570032100")

# RFC 8032 section 7.1, TEST 1: the key that signs the fixture release.
SIGNING_SEED = bytes.fromhex(
    "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
)
SIGNING_PUBLIC_KEY = bytes.fromhex(
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
)
# A second pinned key. Its base64 form contains "//", which a parser that
# strips `//` comments before reading string literals would cut in half.
OTHER_SEED = bytes.fromhex(
    "3d7d34e22310efc10605098b8a1d51d478ef881709c58304fe878e5d3ec0889d"
)
OTHER_PUBLIC_KEY = bytes.fromhex(
    "fefe2ce26bbffd7e311578c5fe66d7a86a0084a326168e0d353401e5b4b68d9e"
)
# RFC 8032 section 7.1, TEST 3: a valid key that is never pinned.
UNTRUSTED_SEED = bytes.fromhex(
    "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7"
)

RELEASE_TAG = "v9.9.9-rc.1"
RUN_ID = "4242424242"
MANIFEST = "notarization-final-byte-manifest.json"
DMG = "maekon-macos-universal.dmg"
PKG = "maekon-macos-universal.pkg"


def _b64(value: bytes) -> str:
    return base64.b64encode(value).decode("ascii")


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class ReleaseFinalBytesTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory(prefix="maekon-final-bytes-test-")
        self.addCleanup(self._temp.cleanup)
        self.root = Path(self._temp.name)
        self.dist = self.root / "dist"
        self.dist.mkdir()
        self.keys = self.root / "trusted_keys.rs"
        self._write_trusted_keys([OTHER_PUBLIC_KEY, SIGNING_PUBLIC_KEY])

        (self.dist / "maekon-linux-x64.tar.gz").write_bytes(b"linux archive bytes\n")
        (self.dist / "sbom.cdx.json").write_bytes(b'{"bomFormat":"CycloneDX"}\n')
        (self.dist / DMG).write_bytes(b"notarized dmg bytes\n")
        (self.dist / PKG).write_bytes(b"notarized pkg bytes\n")
        self._write_manifest()
        for asset in self._assets():
            self._write_checksum(asset)
            self._sign(asset)

    # -- fixture helpers ---------------------------------------------------

    def _assets(self) -> list[Path]:
        return sorted(
            path
            for path in self.dist.iterdir()
            if path.is_file() and not path.name.endswith((".sha256", ".sig"))
        )

    def _write_trusted_keys(self, keys: list[bytes], *, extra: str = "") -> None:
        entries = "".join(
            f'    // key {index} — see "docs/guides/updater-key-rotation.md".\n'
            f'    "{_b64(key)}",\n'
            for index, key in enumerate(keys)
        )
        self.keys.write_text(
            "//! Trusted Ed25519 verification keys for update artifacts.\n"
            "pub(crate) const TRUSTED_PUBLIC_KEYS: &[&str] = &[\n"
            f"{entries}{extra}];\n",
            encoding="utf-8",
        )

    def _write_manifest(self, **overrides: object) -> None:
        manifest: dict[str, object] = {
            "schema_version": 1,
            "release_tag": RELEASE_TAG,
            "source_run_id": RUN_ID,
            "macos_assets": [
                {
                    "name": name,
                    "sha256": _sha256(self.dist / name),
                    "notary_log": f"dist/notary-logs/{name}.notary-log.json",
                }
                for name in (DMG, PKG)
            ],
        }
        manifest.update(overrides)
        (self.dist / MANIFEST).write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

    def _write_checksum(self, asset: Path, *, release_layout: bool = True) -> None:
        # release.yml writes `<hash>  dist/<name>` for most assets and
        # `<hash>  <name>` for the notarized macOS final bytes.
        label = f"dist/{asset.name}" if release_layout else asset.name
        asset.with_name(asset.name + ".sha256").write_text(
            f"{_sha256(asset)}  {label}\n", encoding="utf-8"
        )

    def _sign(self, asset: Path, *, seed: bytes = SIGNING_SEED) -> None:
        private_der = self.root / "private.der"
        signature = self.root / "signature.bin"
        private_der.write_bytes(PRIVATE_DER_PREFIX + seed)
        subprocess.run(
            [
                "openssl",
                "pkeyutl",
                "-sign",
                "-rawin",
                "-inkey",
                str(private_der),
                "-keyform",
                "DER",
                "-in",
                str(asset),
                "-out",
                str(signature),
            ],
            check=True,
            capture_output=True,
        )
        asset.with_name(asset.name + ".sig").write_text(
            _b64(signature.read_bytes()) + "\n", encoding="utf-8"
        )

    def _resign_manifest(self) -> None:
        self._write_checksum(self.dist / MANIFEST)
        self._sign(self.dist / MANIFEST)

    def _run(self, *extra: str, env: dict[str, str] | None = None):
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--dist",
                str(self.dist),
                "--trusted-keys",
                str(self.keys),
                "--release-tag",
                RELEASE_TAG,
                "--source-run-id",
                RUN_ID,
                *extra,
            ],
            check=False,
            capture_output=True,
            text=True,
            env=env,
        )

    def _assert_refused(self, completed, message: str) -> None:
        self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
        self.assertIn("release final-byte verification failed", completed.stderr)
        self.assertIn(message, completed.stderr)

    # -- positive control --------------------------------------------------

    def test_complete_signed_release_passes(self) -> None:
        completed = self._run(
            "--expect-sha256",
            f"{DMG}={_sha256(self.dist / DMG)}",
            "--expect-sha256",
            f"{PKG}={_sha256(self.dist / PKG)}",
        )

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(
            completed.stdout.strip(),
            "release final bytes verified: 5 assets, 15 files, pinned update key #1",
        )

    def test_name_only_checksum_layout_passes(self) -> None:
        for name in (DMG, PKG, MANIFEST):
            self._write_checksum(self.dist / name, release_layout=False)

        self.assertEqual(self._run().returncode, 0)

    # -- every asset is covered ---------------------------------------------

    def test_changed_asset_bytes_are_refused(self) -> None:
        (self.dist / "maekon-linux-x64.tar.gz").write_bytes(b"swapped archive bytes\n")

        self._assert_refused(self._run(), "stale checksum sidecar for maekon-linux-x64.tar.gz")

    def test_changed_asset_with_refreshed_checksum_is_refused_by_signature(self) -> None:
        asset = self.dist / "maekon-linux-x64.tar.gz"
        asset.write_bytes(b"swapped archive bytes\n")
        self._write_checksum(asset)

        self._assert_refused(self._run(), "stale signature sidecar for maekon-linux-x64.tar.gz")

    def test_missing_signature_sidecar_is_refused(self) -> None:
        (self.dist / "sbom.cdx.json.sig").unlink()

        self._assert_refused(self._run(), "missing signature sidecar: sbom.cdx.json.sig")

    def test_missing_checksum_sidecar_is_refused(self) -> None:
        (self.dist / "sbom.cdx.json.sha256").unlink()

        self._assert_refused(self._run(), "missing checksum sidecar: sbom.cdx.json.sha256")

    def test_unsigned_extra_asset_is_refused(self) -> None:
        (self.dist / "unreviewed-extra.bin").write_bytes(b"not part of the release\n")

        self._assert_refused(self._run(), "missing checksum sidecar: unreviewed-extra.bin.sha256")

    def test_orphan_sidecar_is_refused(self) -> None:
        (self.dist / "removed-asset.msi.sig").write_text("AAAA\n", encoding="utf-8")

        self._assert_refused(self._run(), "sidecar without a release asset: removed-asset.msi.sig")

    def test_empty_asset_is_refused(self) -> None:
        (self.dist / "sbom.cdx.json").write_bytes(b"")

        self._assert_refused(self._run(), "empty release asset: sbom.cdx.json")

    def test_directory_inside_release_is_refused(self) -> None:
        (self.dist / "Maekon.app").mkdir()

        self._assert_refused(self._run(), "not a regular file: Maekon.app")

    def test_symlinked_asset_is_refused(self) -> None:
        target = self.root / "outside.bin"
        target.write_bytes(b"outside\n")
        (self.dist / "linked.bin").symlink_to(target)

        self._assert_refused(self._run(), "holds a symlink: linked.bin")

    def test_malformed_checksum_sidecar_is_refused(self) -> None:
        (self.dist / "sbom.cdx.json.sha256").write_text(
            "SHA256(sbom.cdx.json)= abc\n", encoding="utf-8"
        )

        self._assert_refused(self._run(), "malformed checksum sidecar: sbom.cdx.json.sha256")

    def test_noncanonical_signature_sidecar_is_refused(self) -> None:
        (self.dist / "sbom.cdx.json.sig").write_text("not-base64\n", encoding="utf-8")

        self._assert_refused(self._run(), "signature sidecar sbom.cdx.json.sig is not canonical base64")

    # -- signatures are anchored to the pinned keys ---------------------------

    def test_release_signed_by_unpinned_key_is_refused(self) -> None:
        for asset in self._assets():
            self._sign(asset, seed=UNTRUSTED_SEED)

        self._assert_refused(self._run(), "does not verify under any pinned update key")

    def test_single_asset_signed_by_unpinned_key_is_refused(self) -> None:
        self._sign(self.dist / DMG, seed=UNTRUSTED_SEED)

        self._assert_refused(self._run(), f"stale signature sidecar for {DMG}")

    def test_assets_signed_by_two_pinned_keys_are_refused(self) -> None:
        self._sign(self.dist / PKG, seed=OTHER_SEED)

        self._assert_refused(self._run(), f"stale signature sidecar for {PKG}")

    def test_release_signed_by_first_pinned_key_passes(self) -> None:
        for asset in self._assets():
            self._sign(asset, seed=OTHER_SEED)

        completed = self._run()

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("pinned update key #0", completed.stdout)

    def test_signing_key_removed_from_pinned_keys_is_refused(self) -> None:
        self._write_trusted_keys([OTHER_PUBLIC_KEY])

        self._assert_refused(self._run(), "does not verify under any pinned update key")

    def test_missing_trusted_keys_array_is_refused(self) -> None:
        self.keys.write_text("pub(crate) const OTHER: &[&str] = &[];\n", encoding="utf-8")

        self._assert_refused(self._run(), "TRUSTED_PUBLIC_KEYS array not found")

    def test_empty_trusted_keys_array_is_refused(self) -> None:
        self._write_trusted_keys([])

        self._assert_refused(self._run(), "is empty")

    def test_trusted_keys_entry_that_is_not_a_literal_is_refused(self) -> None:
        self._write_trusted_keys([SIGNING_PUBLIC_KEY], extra="    ROTATED_KEY,\n")

        self._assert_refused(self._run(), "holds an entry that is not a string literal")

    def test_trusted_key_of_wrong_length_is_refused(self) -> None:
        self._write_trusted_keys([SIGNING_PUBLIC_KEY[:31]])

        self._assert_refused(self._run(), "must decode to exactly 32 bytes")

    # -- notarized final bytes are the manifest's bytes ------------------------

    def test_manifest_for_another_tag_is_refused(self) -> None:
        self._write_manifest(release_tag="v9.9.9-rc.2")
        self._resign_manifest()

        self._assert_refused(self._run(), "is for release tag 'v9.9.9-rc.2'")

    def test_manifest_from_another_run_is_refused(self) -> None:
        self._write_manifest(source_run_id="1111111111")
        self._resign_manifest()

        self._assert_refused(self._run(), "records source run '1111111111'")

    def test_manifest_with_unsupported_schema_is_refused(self) -> None:
        self._write_manifest(schema_version=2)
        self._resign_manifest()

        self._assert_refused(self._run(), "unsupported schema_version")

    def test_manifest_recording_other_dmg_bytes_is_refused(self) -> None:
        self._write_manifest(
            macos_assets=[
                {"name": DMG, "sha256": "0" * 64},
                {"name": PKG, "sha256": _sha256(self.dist / PKG)},
            ]
        )
        self._resign_manifest()

        self._assert_refused(self._run(), f"notarization manifest checksum mismatch for {DMG}")

    def test_manifest_without_pkg_entry_is_refused(self) -> None:
        self._write_manifest(
            macos_assets=[{"name": DMG, "sha256": _sha256(self.dist / DMG)}]
        )
        self._resign_manifest()

        self._assert_refused(self._run(), "must record exactly")

    def test_missing_manifest_is_refused(self) -> None:
        for suffix in ("", ".sha256", ".sig"):
            (self.dist / (MANIFEST + suffix)).unlink()

        self._assert_refused(self._run(), f"missing release asset: {MANIFEST}")

    def test_missing_dmg_is_refused(self) -> None:
        for suffix in ("", ".sha256", ".sig"):
            (self.dist / (DMG + suffix)).unlink()

        self._assert_refused(self._run(), f"missing release asset: {DMG}")

    # -- bytes accepted earlier in the run are the bytes published --------------

    def test_installer_that_was_not_the_one_smoke_tested_is_refused(self) -> None:
        completed = self._run("--expect-sha256", f"{DMG}={'1' * 64}")

        self._assert_refused(completed, "but the bytes accepted earlier in this run were")

    def test_expected_digest_for_absent_asset_is_refused(self) -> None:
        completed = self._run("--expect-sha256", f"absent.msi={'1' * 64}")

        self._assert_refused(completed, "missing release asset: absent.msi")

    def test_malformed_expected_digest_is_refused(self) -> None:
        completed = self._run("--expect-sha256", f"{DMG}=")

        self._assert_refused(completed, "--expect-sha256 needs NAME=<64 lowercase hex>")

    # -- the verifier does not trust a broken tool -----------------------------

    def _fake_openssl_env(self, exit_code: int) -> dict[str, str]:
        bin_dir = self.root / f"fake-openssl-{exit_code}"
        bin_dir.mkdir()
        fake = bin_dir / "openssl"
        fake.write_text(f"#!/bin/sh\nexit {exit_code}\n", encoding="utf-8")
        fake.chmod(fake.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        return {**os.environ, "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}"}

    def test_tool_that_approves_everything_is_refused(self) -> None:
        completed = self._run(env=self._fake_openssl_env(0))

        self._assert_refused(completed, "OpenSSL accepted a corrupted Ed25519 signature")

    def test_tool_that_cannot_verify_ed25519_is_refused(self) -> None:
        completed = self._run(env=self._fake_openssl_env(1))

        self._assert_refused(completed, "OpenSSL cannot verify a known-good Ed25519 signature")

    def test_missing_tool_is_refused(self) -> None:
        empty_bin = self.root / "empty-bin"
        empty_bin.mkdir()
        self.assertIsNone(shutil.which("openssl", path=str(empty_bin)))

        completed = self._run(env={**os.environ, "PATH": str(empty_bin)})

        self._assert_refused(completed, "OpenSSL is required to verify update signatures")


if __name__ == "__main__":
    unittest.main()
