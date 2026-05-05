#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="${ROOT_DIR}/artifacts/integrity/key-rotation"

mkdir -p "${OUT_DIR}"

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required" >&2
  exit 1
fi

python3 - <<'PY'
import base64
from pathlib import Path

try:
    from nacl.signing import SigningKey
except Exception as exc:
    raise SystemExit(
        "PyNaCl is required for rehearsal. Install: python3 -m pip install pynacl"
    ) from exc

root = Path("artifacts/integrity/key-rotation")
root.mkdir(parents=True, exist_ok=True)

policy_payload = b'{"integrity_policy":"strict","version":1}'
policy_path = root / "policy.json"
policy_path.write_bytes(policy_payload)

old_signing = SigningKey.generate()
new_signing = SigningKey.generate()

old_pub = old_signing.verify_key.encode()
new_pub = new_signing.verify_key.encode()

old_sig = old_signing.sign(policy_payload).signature
new_sig = new_signing.sign(policy_payload).signature

(root / "old_public_key.b64").write_text(base64.b64encode(old_pub).decode("ascii") + "\n")
(root / "new_public_key.b64").write_text(base64.b64encode(new_pub).decode("ascii") + "\n")
(root / "policy.json.old.sig").write_text(base64.b64encode(old_sig).decode("ascii") + "\n")
(root / "policy.json.new.sig").write_text(base64.b64encode(new_sig).decode("ascii") + "\n")

print("Key rotation rehearsal artifacts generated:")
print(f"- {root / 'policy.json'}")
print(f"- {root / 'old_public_key.b64'}")
print(f"- {root / 'new_public_key.b64'}")
print(f"- {root / 'policy.json.old.sig'}")
print(f"- {root / 'policy.json.new.sig'}")
PY

echo "Rehearsal complete. Validate both signatures against policy bundle before cutover."
