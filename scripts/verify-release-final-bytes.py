#!/usr/bin/env python3
"""Fail closed unless a release directory is exactly what may be published.

Every asset must carry a matching SHA-256 sidecar and an Ed25519 update
signature that verifies under one key pinned in the tagged source
(`src-tauri/src/updater/trusted_keys.rs`), and the notarized macOS installers
must be the bytes recorded in `notarization-final-byte-manifest.json`.

Signatures are produced with PyNaCl in the protected signing job. This check
uses the OpenSSL CLI instead, so the implementation that signed cannot vouch
for itself, and it runs in a job that holds no signing material.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


PUBLIC_DER_PREFIX = bytes.fromhex("302a300506032b6570032100")
MANIFEST_NAME = "notarization-final-byte-manifest.json"
MACOS_FINAL_ASSETS = ("maekon-macos-universal.dmg", "maekon-macos-universal.pkg")
SIDECAR_SUFFIXES = (".sha256", ".sig")
SHA256_HEX = re.compile(r"[0-9a-f]{64}")
TRUSTED_KEYS_ARRAY = re.compile(
    r"TRUSTED_PUBLIC_KEYS\s*:\s*&\[\s*&str\s*\]\s*=\s*&\[(?P<body>.*?)\]\s*;",
    re.DOTALL,
)
TRUSTED_KEYS_TOKEN = re.compile(r'"(?P<literal>[^"\\\n]*)"|//[^\n]*')

# RFC 8032 section 7.1, TEST 2. The local OpenSSL has to accept this signature
# and reject a corrupted copy before any of its verdicts on release assets
# count: a tool that cannot do Ed25519, or that approves everything, would
# otherwise look exactly like a clean release.
SELF_TEST_PUBLIC_KEY = bytes.fromhex(
    "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"
)
SELF_TEST_MESSAGE = bytes.fromhex("72")
SELF_TEST_SIGNATURE = bytes.fromhex(
    "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da"
    "085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00"
)


class ReleaseBytesError(Exception):
    """A release directory that must not be published."""


def _decode_canonical_base64(value: str, *, what: str, length: int) -> bytes:
    try:
        decoded = base64.b64decode(value, validate=True)
    except (binascii.Error, ValueError) as exc:
        raise ReleaseBytesError(f"{what} is not canonical base64") from exc
    if base64.b64encode(decoded).decode("ascii") != value:
        raise ReleaseBytesError(f"{what} is not canonical base64")
    if len(decoded) != length:
        raise ReleaseBytesError(
            f"{what} must decode to exactly {length} bytes, got {len(decoded)}"
        )
    return decoded


def load_trusted_keys(path: Path) -> list[bytes]:
    """Read the pinned update keys from the tagged Rust source.

    Anything in the array that is not a string literal or a comment is refused:
    a key this parser cannot see is a key it would silently not check against.
    """
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ReleaseBytesError(f"cannot read trusted keys source: {path}") from exc
    match = TRUSTED_KEYS_ARRAY.search(text)
    if match is None:
        raise ReleaseBytesError(f"TRUSTED_PUBLIC_KEYS array not found in {path}")
    body = match.group("body")
    literals = [
        token.group("literal")
        for token in TRUSTED_KEYS_TOKEN.finditer(body)
        if token.group("literal") is not None
    ]
    remainder = TRUSTED_KEYS_TOKEN.sub("", body)
    if remainder.strip(" \t\r\n,"):
        raise ReleaseBytesError(
            f"TRUSTED_PUBLIC_KEYS in {path} holds an entry that is not a string literal"
        )
    if not literals:
        raise ReleaseBytesError(f"TRUSTED_PUBLIC_KEYS in {path} is empty")
    keys: list[bytes] = []
    for index, literal in enumerate(literals):
        key = _decode_canonical_base64(
            literal, what=f"TRUSTED_PUBLIC_KEYS[{index}]", length=32
        )
        if key not in keys:
            keys.append(key)
    return keys


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class _OpenSslEd25519:
    def __init__(self, workdir: Path) -> None:
        self._workdir = workdir
        self._counter = 0
        if shutil.which("openssl") is None:
            raise ReleaseBytesError("OpenSSL is required to verify update signatures")

    def verify(self, public_key: bytes, payload: Path, signature: bytes) -> bool:
        self._counter += 1
        public_der = self._workdir / f"public-{self._counter}.der"
        signature_file = self._workdir / f"signature-{self._counter}.bin"
        public_der.write_bytes(PUBLIC_DER_PREFIX + public_key)
        signature_file.write_bytes(signature)
        completed = subprocess.run(
            [
                "openssl",
                "pkeyutl",
                "-verify",
                "-rawin",
                "-pubin",
                "-inkey",
                str(public_der),
                "-keyform",
                "DER",
                "-in",
                str(payload),
                "-sigfile",
                str(signature_file),
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        return completed.returncode == 0

    def self_test(self) -> None:
        message = self._workdir / "self-test-message.bin"
        message.write_bytes(SELF_TEST_MESSAGE)
        if not self.verify(SELF_TEST_PUBLIC_KEY, message, SELF_TEST_SIGNATURE):
            raise ReleaseBytesError(
                "OpenSSL cannot verify a known-good Ed25519 signature on this host"
            )
        corrupted = self._workdir / "self-test-corrupted.bin"
        corrupted.write_bytes(bytes([SELF_TEST_MESSAGE[0] ^ 1]))
        if self.verify(SELF_TEST_PUBLIC_KEY, corrupted, SELF_TEST_SIGNATURE):
            raise ReleaseBytesError(
                "OpenSSL accepted a corrupted Ed25519 signature; its verdicts cannot be trusted"
            )


def _read_first_token(path: Path, *, what: str) -> str:
    try:
        tokens = path.read_text(encoding="utf-8").split()
    except (OSError, UnicodeDecodeError) as exc:
        raise ReleaseBytesError(f"cannot read {what}: {path.name}") from exc
    if not tokens:
        raise ReleaseBytesError(f"empty {what}: {path.name}")
    return tokens[0]


def _classify(dist: Path) -> tuple[list[Path], set[str]]:
    if not dist.is_dir():
        raise ReleaseBytesError(f"release directory not found: {dist}")
    assets: list[Path] = []
    sidecars: set[str] = set()
    for entry in sorted(dist.iterdir(), key=lambda item: item.name):
        if entry.is_symlink():
            raise ReleaseBytesError(f"release directory holds a symlink: {entry.name}")
        if not entry.is_file():
            raise ReleaseBytesError(
                f"release directory holds something that is not a regular file: {entry.name}"
            )
        if entry.name.endswith(SIDECAR_SUFFIXES):
            sidecars.add(entry.name)
        else:
            assets.append(entry)
    if not assets:
        raise ReleaseBytesError("release directory holds no assets")
    expected_sidecars = {
        f"{asset.name}{suffix}" for asset in assets for suffix in SIDECAR_SUFFIXES
    }
    orphans = sorted(sidecars - expected_sidecars)
    if orphans:
        raise ReleaseBytesError(f"sidecar without a release asset: {orphans[0]}")
    return assets, sidecars


def _verify_manifest(
    dist: Path,
    digests: dict[str, str],
    *,
    release_tag: str,
    source_run_id: str,
) -> None:
    if MANIFEST_NAME not in digests:
        raise ReleaseBytesError(f"missing release asset: {MANIFEST_NAME}")
    for name in MACOS_FINAL_ASSETS:
        if name not in digests:
            raise ReleaseBytesError(f"missing release asset: {name}")
    try:
        manifest = json.loads((dist / MANIFEST_NAME).read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ReleaseBytesError(f"{MANIFEST_NAME} is not valid JSON") from exc
    if not isinstance(manifest, dict):
        raise ReleaseBytesError(f"{MANIFEST_NAME} must be a JSON object")
    if manifest.get("schema_version") != 1:
        raise ReleaseBytesError(f"{MANIFEST_NAME} has an unsupported schema_version")
    if manifest.get("release_tag") != release_tag:
        raise ReleaseBytesError(
            f"{MANIFEST_NAME} is for release tag {manifest.get('release_tag')!r}, expected {release_tag!r}"
        )
    recorded_run_id = manifest.get("source_run_id")
    if isinstance(recorded_run_id, bool) or str(recorded_run_id) != source_run_id:
        raise ReleaseBytesError(
            f"{MANIFEST_NAME} records source run {recorded_run_id!r}, expected {source_run_id!r}"
        )
    entries = manifest.get("macos_assets")
    if not isinstance(entries, list) or not all(isinstance(item, dict) for item in entries):
        raise ReleaseBytesError(f"{MANIFEST_NAME} macos_assets must be a list of objects")
    recorded = [item.get("name") for item in entries]
    if sorted(str(name) for name in recorded) != sorted(MACOS_FINAL_ASSETS):
        raise ReleaseBytesError(
            f"{MANIFEST_NAME} must record exactly {', '.join(MACOS_FINAL_ASSETS)}"
        )
    for item in entries:
        name = item["name"]
        if item.get("sha256") != digests[name]:
            raise ReleaseBytesError(f"notarization manifest checksum mismatch for {name}")


def verify_release_dir(
    dist: Path,
    *,
    trusted_keys: list[bytes],
    release_tag: str,
    source_run_id: str,
    expected_digests: dict[str, str],
) -> dict[str, object]:
    assets, sidecars = _classify(dist)

    digests: dict[str, str] = {}
    signatures: dict[str, bytes] = {}
    for asset in assets:
        if asset.stat().st_size == 0:
            raise ReleaseBytesError(f"empty release asset: {asset.name}")
        checksum_name = f"{asset.name}.sha256"
        signature_name = f"{asset.name}.sig"
        if checksum_name not in sidecars:
            raise ReleaseBytesError(f"missing checksum sidecar: {checksum_name}")
        if signature_name not in sidecars:
            raise ReleaseBytesError(f"missing signature sidecar: {signature_name}")
        recorded = _read_first_token(dist / checksum_name, what="checksum sidecar")
        if SHA256_HEX.fullmatch(recorded) is None:
            raise ReleaseBytesError(f"malformed checksum sidecar: {checksum_name}")
        actual = _sha256_file(asset)
        if actual != recorded:
            raise ReleaseBytesError(
                f"stale checksum sidecar for {asset.name}: file is {actual}, sidecar says {recorded}"
            )
        digests[asset.name] = actual
        signatures[asset.name] = _decode_canonical_base64(
            _read_first_token(dist / signature_name, what="signature sidecar"),
            what=f"signature sidecar {signature_name}",
            length=64,
        )

    for name, expected in sorted(expected_digests.items()):
        if name not in digests:
            raise ReleaseBytesError(f"missing release asset: {name}")
        if digests[name] != expected:
            raise ReleaseBytesError(
                f"{name} is {digests[name]}, but the bytes accepted earlier in this run were {expected}"
            )

    _verify_manifest(dist, digests, release_tag=release_tag, source_run_id=source_run_id)

    with tempfile.TemporaryDirectory(prefix="maekon-release-final-bytes-") as temp_dir:
        openssl = _OpenSslEd25519(Path(temp_dir))
        openssl.self_test()
        # One key signs a release. Find it on the smallest asset, then hold
        # every other asset to that same key.
        probe = min(assets, key=lambda item: (item.stat().st_size, item.name))
        signing_key_index = next(
            (
                index
                for index, key in enumerate(trusted_keys)
                if openssl.verify(key, probe, signatures[probe.name])
            ),
            None,
        )
        if signing_key_index is None:
            raise ReleaseBytesError(
                f"signature for {probe.name} does not verify under any pinned update key"
            )
        signing_key = trusted_keys[signing_key_index]
        for asset in assets:
            if not openssl.verify(signing_key, asset, signatures[asset.name]):
                raise ReleaseBytesError(
                    f"stale signature sidecar for {asset.name}: it does not verify under "
                    f"the pinned update key that signed {probe.name}"
                )

    return {
        "assets": len(assets),
        "files": len(assets) + len(sidecars),
        "trusted_key_index": signing_key_index,
    }


def _parse_expected(values: list[str]) -> dict[str, str]:
    expected: dict[str, str] = {}
    for value in values:
        name, separator, digest = value.partition("=")
        if not separator or not name or SHA256_HEX.fullmatch(digest) is None:
            raise ReleaseBytesError(
                f"--expect-sha256 needs NAME=<64 lowercase hex>, got {value!r}"
            )
        if expected.setdefault(name, digest) != digest:
            raise ReleaseBytesError(f"--expect-sha256 given twice for {name}")
    return expected


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dist", required=True, type=Path)
    parser.add_argument("--trusted-keys", required=True, type=Path)
    parser.add_argument("--release-tag", required=True)
    parser.add_argument("--source-run-id", required=True)
    parser.add_argument("--expect-sha256", action="append", default=[], metavar="NAME=SHA256")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        summary = verify_release_dir(
            args.dist,
            trusted_keys=load_trusted_keys(args.trusted_keys),
            release_tag=args.release_tag,
            source_run_id=args.source_run_id,
            expected_digests=_parse_expected(args.expect_sha256),
        )
    except ReleaseBytesError as exc:
        print(f"release final-byte verification failed: {exc}", file=sys.stderr)
        return 1
    print(
        "release final bytes verified: "
        f"{summary['assets']} assets, {summary['files']} files, "
        f"pinned update key #{summary['trusted_key_index']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
