#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TRUSTED_KEYS_PATH="${TRUSTED_KEYS_PATH:-${ROOT_DIR}/src-tauri/src/updater/trusted_keys.rs}"

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required" >&2
  exit 1
fi

if [[ -z "${UPDATE_SIGNING_PRIVATE_KEY_B64:-}" ]]; then
  if [[ -t 0 ]]; then
    read -r -s -p "UPDATE_SIGNING_PRIVATE_KEY_B64: " UPDATE_SIGNING_PRIVATE_KEY_B64
    echo
    export UPDATE_SIGNING_PRIVATE_KEY_B64
  else
    echo "Set UPDATE_SIGNING_PRIVATE_KEY_B64 or run interactively to enter it securely." >&2
    exit 2
  fi
fi

export TRUSTED_KEYS_PATH

python3 - <<'PY'
import base64
import os
import re
from pathlib import Path

try:
    from nacl.signing import SigningKey
except Exception as exc:
    raise SystemExit(
        "PyNaCl is required. Install: python3 -m pip install pynacl"
    ) from exc

seed_b64 = os.environ["UPDATE_SIGNING_PRIVATE_KEY_B64"].strip()
try:
    seed = base64.b64decode(seed_b64, validate=True)
except Exception as exc:
    raise SystemExit("UPDATE_SIGNING_PRIVATE_KEY_B64 is not valid base64") from exc

if len(seed) != 32:
    raise SystemExit(
        f"UPDATE_SIGNING_PRIVATE_KEY_B64 must decode to 32 bytes, got {len(seed)}"
    )

trusted_path = Path(os.environ["TRUSTED_KEYS_PATH"])
trusted_source = trusted_path.read_text(encoding="utf-8")
trusted_keys = re.findall(r'"([A-Za-z0-9+/]+={0,2})"', trusted_source)
trusted_keys = [key for key in trusted_keys if len(base64.b64decode(key)) == 32]

if not trusted_keys:
    raise SystemExit(f"No 32-byte trusted public keys found in {trusted_path}")

expected = os.environ.get("EXPECTED_UPDATE_PUBLIC_KEY_B64", "").strip() or trusted_keys[0]
derived = base64.b64encode(SigningKey(seed).verify_key.encode()).decode("ascii")

if derived != expected:
    print("Update signing key mismatch:", flush=True)
    print(f"- derived public key: {derived}", flush=True)
    print(f"- expected public key: {expected}", flush=True)
    raise SystemExit(1)

if derived not in trusted_keys:
    print("Derived public key matches the expected override but is not in TRUSTED_PUBLIC_KEYS.", flush=True)
    raise SystemExit(1)

print("Update signing key matches TRUSTED_PUBLIC_KEYS[0].")
PY
