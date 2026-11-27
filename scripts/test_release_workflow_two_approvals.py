#!/usr/bin/env python3
"""Contract tests for the two-approval release workflow.

A release waits for a reviewer twice: once before anything is built
(verify-version) and once before anything is signed (sign-notarize-macos).
These tests hold release.yml to that shape and to the reasons it is safe.

- Structure: which jobs wait for a reviewer, which jobs can read which
  secrets, and what the publishing job depends on.
- Behaviour: the bash that hands bytes from one job to the next is taken out
  of release.yml and executed here against stubbed `gh`, `xcrun` and `lipo`,
  so the text under test is the workflow's own.
"""

from __future__ import annotations

import base64
import hashlib
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
import zipfile
from dataclasses import dataclass
from pathlib import Path

CLIENT_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_DIR = CLIENT_ROOT / ".github" / "workflows"
RELEASE_WORKFLOW = WORKFLOW_DIR / "release.yml"
NOTARIZE_WORKFLOW = WORKFLOW_DIR / "notarize-macos-release-assets.yml"
VERIFIER = Path(__file__).with_name("verify-release-final-bytes.py")

REVIEWED_ENVIRONMENT = "release-signing"
# Environments with no required reviewer. Any other environment name counts as
# an approval, so a new one has to be classified here on purpose.
UNREVIEWED_ENVIRONMENTS = {"desktop-smoke"}
APPROVAL_JOBS = ["verify-version", "sign-notarize-macos"]
SIGNING_JOB = "sign-notarize-macos"
INSTALLER_SMOKE_JOB = "macos-installer-smoke"
PUBLISH_JOB = "create-release"
# What a job without a reviewer may read: the run's own token and the landing
# page deploy hook. Neither can sign, notarize or stand in for a signature.
UNREVIEWED_SECRETS = {"GITHUB_TOKEN", "MAEKON_LANDING_DEPLOY_HOOK"}
SIGNING_JOB_SECRETS = {
    "MACOS_APP_CERT_P12_B64",
    "MACOS_APP_CERT_PASSWORD",
    "MACOS_APP_SIGNING_IDENTITY",
    "MACOS_INSTALLER_CERT_P12_B64",
    "MACOS_INSTALLER_CERT_PASSWORD",
    "MACOS_INSTALLER_SIGNING_IDENTITY",
    "MACOS_NOTARY_APPLE_ID",
    "MACOS_NOTARY_APP_PASSWORD",
    "MACOS_NOTARY_TEAM_ID",
    "UPDATE_SIGNING_PRIVATE_KEY_B64",
}
REHEARSAL_GUARD = "env.RELEASE_REHEARSAL != 'true'"

SECRET = re.compile(r"\bsecrets\.([A-Za-z0-9_]+)")
NETWORK_INSTALL = re.compile(r"\b(?:brew install|pip install|npm (?:ci|install)|pnpm install|cargo install)\b|\b(?:curl|wget) ")
# Anything that changes what the public can see.
OUTWARD = re.compile(
    r"gh release (?:create|upload|edit|delete)|--method (?:DELETE|POST|PATCH|PUT)|curl -X POST|attest-build-provenance@"
)

MANIFEST = "notarization-final-byte-manifest.json"
DMG = "maekon-macos-universal.dmg"
PKG = "maekon-macos-universal.pkg"
UNIVERSAL = "maekon-macos-universal.tar.gz"
RELEASE_TAG = "v9.9.9-rc.1"
RUN_ID = "4242424242"
REPOSITORY = "example/maekon-client"

PRIVATE_DER_PREFIX = "302e020100300506032b657004220420"
PUBLIC_DER_PREFIX_LENGTH = 12
# RFC 8032 section 7.1, TEST 1: the key the fixture client pins.
SIGNING_SEED = bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
SIGNING_PUBLIC_KEY = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
# RFC 8032 section 7.1, TEST 3: a valid key the fixture client does not pin.
UNPINNED_SEED = bytes.fromhex("c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7")
# RFC 8032 section 7.1, TEST 2 and TEST 3: (seed, public key, message,
# signature). TEST 1 signs the empty message, which `openssl pkeyutl -rawin`
# cannot read, so the stand-in is held to the two vectors that sign bytes.
RFC8032_VECTORS = (
    (
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da"
        "085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        UNPINNED_SEED.hex(),
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac"
        "18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
)


# ── a small reader for the workflow's own layout ────────────────────────────


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def _block_scalar(lines: list[str], index: int) -> str:
    """Body of the `key: |` block that opens on lines[index]."""
    indent = _indent(lines[index])
    body: list[str] = []
    for line in lines[index + 1 :]:
        if line.strip() and _indent(line) <= indent:
            break
        body.append(line[indent + 2 :] if line.strip() else "")
    while body and not body[-1]:
        body.pop()
    return "\n".join(body) + "\n"


def _nested(lines: list[str], index: int) -> list[str]:
    """Lines nested under the mapping key on lines[index]."""
    indent = _indent(lines[index])
    nested: list[str] = []
    for line in lines[index + 1 :]:
        if line.strip() and _indent(line) <= indent:
            break
        nested.append(line)
    return nested


def _props(lines: list[str], indent: int) -> dict[str, tuple[int, str]]:
    props: dict[str, tuple[int, str]] = {}
    pattern = re.compile(rf"^ {{{indent}}}([A-Za-z0-9_-]+):(?: (.*))?$")
    for index, line in enumerate(lines):
        match = pattern.match(line)
        if match and match.group(1) not in props:
            props[match.group(1)] = (index, (match.group(2) or "").strip())
    return props


@dataclass
class Step:
    lines: list[str]

    def __post_init__(self) -> None:
        self._props = _props(self.lines, 8)

    def prop(self, key: str) -> str | None:
        return self._props[key][1] if key in self._props else None

    @property
    def name(self) -> str:
        return self.prop("name") or self.prop("uses") or ""

    @property
    def text(self) -> str:
        return "\n".join(self.lines)

    @property
    def run(self) -> str | None:
        if "run" not in self._props:
            return None
        index, value = self._props["run"]
        return _block_scalar(self.lines, index) if value == "|" else value + "\n"

    @property
    def env_lines(self) -> list[str]:
        return _nested(self.lines, self._props["env"][0]) if "env" in self._props else []


@dataclass
class Job:
    job_id: str
    header: list[str]
    steps: list[Step]

    def __post_init__(self) -> None:
        self._props = _props(self.header, 4)

    def prop(self, key: str) -> str | None:
        return self._props[key][1] if key in self._props else None

    @property
    def environment(self) -> str | None:
        if "environment" not in self._props:
            return None
        index, value = self._props["environment"]
        if value:
            return value
        # `environment:` written as a mapping.
        nested = _props(_nested(self.header, index), 6)
        return nested["name"][1] if "name" in nested else "<unnamed>"

    @property
    def needs(self) -> list[str]:
        value = self.prop("needs") or ""
        return [item.strip() for item in value.strip("[]").split(",") if item.strip()]

    @property
    def text(self) -> str:
        return "\n".join(self.header + [line for step in self.steps for line in step.lines])

    @property
    def secrets(self) -> set[str]:
        return set(SECRET.findall(self.text))

    def step(self, name: str) -> Step:
        matches = [step for step in self.steps if step.name == name]
        assert len(matches) == 1, f"{self.job_id}: expected one step named {name!r}, found {len(matches)}"
        return matches[0]

    def index(self, name: str) -> int:
        return self.steps.index(self.step(name))


def parse_jobs(text: str) -> dict[str, Job]:
    lines = text.split("\n")
    jobs: dict[str, Job] = {}
    header: list[str] | None = None
    steps: list[list[str]] | None = None
    job_id = ""

    def close() -> None:
        if header is not None:
            jobs[job_id] = Job(job_id, header, [Step(step) for step in steps or []])

    for line in lines[lines.index("jobs:") + 1 :]:
        job_match = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if job_match:
            close()
            job_id, header, steps = job_match.group(1), [], None
            continue
        if header is None:
            continue
        if steps is None:
            if line == "    steps:":
                steps = []
            else:
                header.append(line)
            continue
        step_match = re.match(r"^      - (.*)$", line)
        if step_match:
            steps.append(["        " + step_match.group(1)])
        elif steps:
            steps[-1].append(line)
    close()
    return jobs


def triggers(text: str) -> set[str]:
    lines = text.split("\n")
    found = _props(_nested(lines, lines.index("on:")), 2)
    return set(found)


# ── structure ───────────────────────────────────────────────────────────────


class ReleaseApprovalStructureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.text = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        cls.jobs = parse_jobs(cls.text)

    def test_the_reader_sees_every_job_and_its_steps(self) -> None:
        # Without this, a reader that stopped matching would pass every test
        # below by examining nothing.
        declared = re.findall(r"(?m)^  ([A-Za-z0-9_-]+):\s*$", self.text.split("\njobs:\n", 1)[1])
        self.assertEqual(list(self.jobs), declared)
        self.assertGreaterEqual(len(self.jobs), 9)
        for job in self.jobs.values():
            self.assertTrue(job.steps, f"{job.job_id} has no steps")
        body = self.text.split("\njobs:\n", 1)[1]
        self.assertEqual(body.count("\n      - "), sum(len(job.steps) for job in self.jobs.values()))
        self.assertEqual(
            len(re.findall(r"(?m)^        run:", body)),
            sum(1 for job in self.jobs.values() for step in job.steps if step.run is not None),
        )

    def test_exactly_two_jobs_wait_for_a_reviewer(self) -> None:
        waiting = [
            job.job_id
            for job in self.jobs.values()
            if job.environment is not None and job.environment not in UNREVIEWED_ENVIRONMENTS
        ]
        self.assertEqual(waiting, APPROVAL_JOBS)
        for job_id in APPROVAL_JOBS:
            self.assertEqual(self.jobs[job_id].environment, REVIEWED_ENVIRONMENT)

    def test_notarization_workflow_only_starts_by_hand(self) -> None:
        # It used to start from every finished Release run and wait for two
        # more approvals. Notarization is part of the release run now.
        self.assertEqual(triggers(NOTARIZE_WORKFLOW.read_text(encoding="utf-8")), {"workflow_dispatch"})

    def test_no_reviewed_workflow_starts_from_a_finished_release(self) -> None:
        examined = 0
        for workflow in sorted(WORKFLOW_DIR.glob("*.yml")):
            text = workflow.read_text(encoding="utf-8")
            examined += 1
            if "workflow_run" not in triggers(text):
                continue
            lines = text.split("\n")
            on_block = "\n".join(_nested(lines, lines.index("on:")))
            if not re.search(r"""workflows:.*["']Release["']""", on_block):
                continue
            environments = set(re.findall(r"(?m)^\s+environment:\s*(\S+)", text))
            self.assertLessEqual(environments, UNREVIEWED_ENVIRONMENTS, workflow.name)
        self.assertGreater(examined, 5)

    def test_only_reviewed_jobs_can_read_signing_secrets(self) -> None:
        for job in self.jobs.values():
            if job.environment == REVIEWED_ENVIRONMENT:
                continue
            self.assertEqual(job.secrets - UNREVIEWED_SECRETS, set(), job.job_id)

    def test_signing_job_scopes_every_secret_to_the_step_that_uses_it(self) -> None:
        job = self.jobs[SIGNING_JOB]
        self.assertEqual(set(SECRET.findall("\n".join(job.header))), set())
        used: set[str] = set()
        for step in job.steps:
            names = set(SECRET.findall(step.text))
            self.assertEqual(names, set(SECRET.findall("\n".join(step.env_lines))), step.name)
            if names:
                # A secret goes to this workflow's own script, never to an action.
                self.assertIsNone(step.prop("uses"), step.name)
                self.assertIsNotNone(step.run, step.name)
            used |= names
        self.assertEqual(used, SIGNING_JOB_SECRETS)

    def test_signing_job_runs_only_first_party_actions(self) -> None:
        job = self.jobs[SIGNING_JOB]
        uses = [step.prop("uses") for step in job.steps if step.prop("uses")]
        self.assertEqual({value.split("@")[0] for value in uses}, {"actions/checkout", "actions/upload-artifact"})
        for value in uses:
            self.assertRegex(value, r"^actions/[a-z-]+@[0-9a-f]{40}\b")

    def test_signing_job_never_launches_or_fetches_code_after_a_key_arrives(self) -> None:
        job = self.jobs[SIGNING_JOB]
        # It works from the release tree alone and leaves launching the app to
        # the smoke jobs, which hold no key.
        steps_text = "\n".join(step.text for step in job.steps)
        for forbidden in (".workflow-tools", "release-installer-smoke-macos.sh", "release-reliability-smoke"):
            self.assertNotIn(forbidden, steps_text)
        installs = [index for index, step in enumerate(job.steps) if NETWORK_INSTALL.search(step.run or "")]
        self.assertEqual(
            [job.steps[index].name for index in installs],
            ["Install create-dmg", "Install the update-signing library"],
        )
        first_secret = next(index for index, step in enumerate(job.steps) if SECRET.search(step.text))
        self.assertLess(max(installs), first_secret)

    def test_signing_job_waits_for_every_smoke_and_the_payload(self) -> None:
        job = self.jobs[SIGNING_JOB]
        self.assertIsNone(job.prop("if"))
        for need in ("verify-version", "build-macos-universal", "release-reliability-smoke", "assemble-release-payload"):
            self.assertIn(need, job.needs)

    def test_signing_job_reports_its_own_failure(self) -> None:
        job = self.jobs[SIGNING_JOB]
        reporters = [step for step in job.steps if step.prop("if") == "failure()"]
        self.assertEqual(len(reporters), 1)
        self.assertIn("MAEKON_NOTARIZATION_FAILURE_V1 workflow=release.yml", reporters[0].run or "")
        # Keys are removed even when a step in between fails.
        cleanup = job.steps[-1]
        self.assertEqual(cleanup.prop("if"), "always()")
        self.assertIn('security delete-keychain "$KEYCHAIN_PATH"', cleanup.run or "")
        self.assertIn('security delete-keychain "$NOTARY_KEYCHAIN_PATH"', cleanup.run or "")

    def test_installer_smoke_launches_final_bytes_without_any_secret(self) -> None:
        job = self.jobs[INSTALLER_SMOKE_JOB]
        self.assertEqual(job.environment, "desktop-smoke")
        self.assertEqual(job.secrets, set())
        self.assertIn(SIGNING_JOB, job.needs)
        self.assertIsNone(job.prop("if"))
        download = job.step("Download the final macOS installers")
        for output in ("dmg_artifact_id", "dmg_artifact_digest", "pkg_artifact_id", "pkg_artifact_digest"):
            self.assertIn(f"needs.{SIGNING_JOB}.outputs.{output}", download.text)
        self.assertLess(job.index("Download the final macOS installers"), job.index("Run macOS installer smoke script"))
        self.assertIn("release-installer-smoke-macos.sh --assets-dir smoke-installers", job.text)

    def test_publishing_needs_signing_and_the_final_byte_smoke(self) -> None:
        job = self.jobs[PUBLISH_JOB]
        self.assertIsNone(job.environment)
        self.assertEqual(job.secrets - UNREVIEWED_SECRETS, set())
        condition = job.prop("if") or ""
        for need in (SIGNING_JOB, INSTALLER_SMOKE_JOB, "release-reliability-smoke", "assemble-release-payload"):
            self.assertIn(need, job.needs)
            self.assertIn(f"needs.{need}.result == 'success'", condition)
        self.assertNotIn("||", condition)
        self.assertNotIn("always()", condition)

        gate = job.index("Verify final bytes against the pinned update keys")
        publish = job.index("Create or update Release")
        readback = job.index("Verify published release bytes")
        self.assertLess(job.index("Download the signed final release set"), gate)
        self.assertLess(gate, publish)
        self.assertEqual(readback, publish + 1)
        for step in (job.steps[gate], job.steps[readback]):
            for expected in (
                "python3 .workflow-tools/scripts/verify-release-final-bytes.py",
                "--trusted-keys src-tauri/src/updater/trusted_keys.rs",
                '--release-tag "$RELEASE_TAG_NAME"',
                '--source-run-id "$GITHUB_RUN_ID"',
                f'--expect-sha256 "{DMG}=$SMOKED_DMG_SHA256"',
                f'--expect-sha256 "{PKG}=$SMOKED_PKG_SHA256"',
            ):
                self.assertIn(expected, step.run or "", step.name)
            self.assertIn(f"needs.{INSTALLER_SMOKE_JOB}.outputs.dmg_sha256", step.text)
            self.assertIn(f"needs.{INSTALLER_SMOKE_JOB}.outputs.pkg_sha256", step.text)
        # The gate may not be skipped or softened.
        self.assertIsNone(job.steps[gate].prop("if"))
        self.assertIsNone(job.steps[gate].prop("continue-on-error"))
        self.assertNotIn("continue-on-error", self.text)

    def test_bytes_cross_jobs_by_artifact_id_and_digest(self) -> None:
        for job_id, step_name, source in (
            (SIGNING_JOB, "Download the unsigned release payload", "assemble-release-payload"),
            (INSTALLER_SMOKE_JOB, "Download the final macOS installers", SIGNING_JOB),
            (PUBLISH_JOB, "Download the signed final release set", SIGNING_JOB),
        ):
            step = self.jobs[job_id].step(step_name)
            self.assertIn(f"needs.{source}.outputs.", step.text)
            self.assertIn("/actions/artifacts/$", step.run or "")
            self.assertIn("shasum -a 256", step.run or "")
            self.assertNotIn("gh run download", step.run or "")
            # Values reach the script through env, never by interpolation.
            self.assertNotIn("${{", step.run or "", step_name)

    def test_a_rehearsal_cannot_publish(self) -> None:
        self.assertIn(
            "  RELEASE_REHEARSAL: ${{ github.event_name == 'workflow_dispatch' && inputs.rehearsal && 'true' || 'false' }}",
            self.text.split("\n"),
        )
        outward = [
            (job.job_id, step)
            for job in self.jobs.values()
            for step in job.steps
            if OUTWARD.search(step.text)
        ]
        self.assertEqual(
            [(job_id, step.name) for job_id, step in outward],
            [
                (PUBLISH_JOB, "Attest release artifacts provenance"),
                (PUBLISH_JOB, "Create or update Release"),
                (PUBLISH_JOB, "Verify published release bytes"),
                (PUBLISH_JOB, "Trigger maekon-landing redeploy"),
            ],
        )
        for _, step in outward:
            self.assertIn(REHEARSAL_GUARD, step.prop("if") or "", step.name)


# ── behaviour ───────────────────────────────────────────────────────────────

GH_STUB = r'''#!/usr/bin/env python3
import os
import re
import shutil
import sys
from pathlib import Path

state = Path(os.environ["STUB_STATE"])
args = sys.argv[1:]
with (state / "gh.log").open("a", encoding="utf-8") as log:
    log.write(" ".join(args) + "\n")


def fail(message):
    print(f"gh stub: {message}", file=sys.stderr)
    sys.exit(1)


if args[:1] == ["api"]:
    rest = args[1:]
    path = next((arg for arg in rest if arg.startswith("repos/")), "")
    if "--method" in rest:
        if rest[rest.index("--method") + 1] != "DELETE":
            fail("unexpected method")
        with (state / "deleted.log").open("a", encoding="utf-8") as log:
            log.write(path + "\n")
        sys.exit(0)
    artifact = re.fullmatch(r"repos/[^/]+/[^/]+/actions/artifacts/(\d+)/zip", path)
    if artifact:
        source = state / f"artifact-{artifact.group(1)}.zip"
        if not source.is_file():
            fail("artifact not found")
        sys.stdout.buffer.write(source.read_bytes())
        sys.exit(0)
    if re.fullmatch(r"repos/[^/]+/[^/]+/releases/tags/[^/]+", path):
        release_id = state / "release-id"
        if not release_id.is_file():
            fail("release not found")
        print(release_id.read_text(encoding="utf-8").strip())
        sys.exit(0)
    fail(f"unexpected api path {path!r}")

if args[:2] == ["release", "download"]:
    published = state / "published"
    if not published.is_dir():
        fail("release not found")
    target = Path(args[args.index("--dir") + 1])
    for item in sorted(published.iterdir()):
        shutil.copy2(item, target / item.name)
    sys.exit(0)

if args[:2] == ["release", "view"]:
    listing = state / "release-assets.txt"
    if not listing.is_file():
        fail("release not found")
    sys.stdout.write(listing.read_text(encoding="utf-8"))
    sys.exit(0)

fail("unexpected command")
'''

XCRUN_STUB = r"""#!/usr/bin/env bash
echo "$*" >> "$STUB_STATE/xcrun.log"
if [[ "${1:-} ${2:-}" == "stapler validate" && -f "${3:-}" ]]; then
  if [[ "$(basename "$3")" == "${XCRUN_UNSTAPLED:-}" ]]; then
    echo "$3 does not have a ticket stapled to it." >&2
    exit 65
  fi
  exit 0
fi
exit 64
"""

LIPO_STUB = r"""#!/usr/bin/env bash
if [[ "${1:-}" == "-archs" && -f "${2:-}" ]]; then
  echo "${LIPO_ARCHS:-x86_64 arm64}"
  exit 0
fi
exit 64
"""

# Stands in for PyNaCl with the same Ed25519, computed by OpenSSL, so the
# signing step runs here without a network install.
NACL_SIGNING_STUB = r'''import subprocess
import tempfile
from pathlib import Path

_PRIVATE_DER_PREFIX = bytes.fromhex("302e020100300506032b657004220420")


class _Signed:
    def __init__(self, signature):
        self.signature = signature


class VerifyKey:
    def __init__(self, raw):
        self._raw = bytes(raw)

    def __bytes__(self):
        return self._raw


class SigningKey:
    def __init__(self, seed):
        self._seed = bytes(seed)
        if len(self._seed) != 32:
            raise ValueError("The seed must be exactly 32 bytes long")

    def _openssl(self, *args, payload=None):
        with tempfile.TemporaryDirectory() as temp:
            key = Path(temp) / "private.der"
            key.write_bytes(_PRIVATE_DER_PREFIX + self._seed)
            command = ["openssl", *args, "-inkey" if args[0] == "pkeyutl" else "-in", str(key)]
            if args[0] == "pkeyutl":
                message = Path(temp) / "message.bin"
                message.write_bytes(payload)
                command += ["-keyform", "DER", "-in", str(message)]
            else:
                command += ["-inform", "DER", "-pubout", "-outform", "DER"]
            return subprocess.run(command, check=True, capture_output=True).stdout

    @property
    def verify_key(self):
        return VerifyKey(self._openssl("pkey")[-32:])

    def sign(self, payload):
        return _Signed(self._openssl("pkeyutl", "-sign", "-rawin", payload=bytes(payload)))
'''


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _b64(value: bytes) -> str:
    return base64.b64encode(value).decode("ascii")


def _write_zip(target: Path, source: Path, *, extra: list[zipfile.ZipInfo | tuple[zipfile.ZipInfo, bytes]] = ()) -> str:
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(source.rglob("*")):
            if path.is_file():
                archive.write(path, path.relative_to(source).as_posix())
        for entry in extra:
            info, data = entry if isinstance(entry, tuple) else (entry, b"")
            archive.writestr(info, data)
    return _sha256(target)


def _symlink_entry(name: str) -> tuple[zipfile.ZipInfo, bytes]:
    info = zipfile.ZipInfo(name)
    info.create_system = 3
    info.external_attr = (stat.S_IFLNK | 0o777) << 16
    return info, b"/etc/hostname"


class StepSandbox(unittest.TestCase):
    """Runs one `run:` block of release.yml the way `shell: bash` does."""

    jobs: dict[str, Job]

    @classmethod
    def setUpClass(cls) -> None:
        cls.jobs = parse_jobs(RELEASE_WORKFLOW.read_text(encoding="utf-8"))

    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="maekon-release-steps-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.workspace = self.root / "workspace"
        self.runner_temp = self.root / "runner-temp"
        self.state = self.root / "state"
        self.bin = self.root / "bin"
        for directory in (self.workspace, self.runner_temp, self.state, self.bin):
            directory.mkdir()
        for name, body in (("gh", GH_STUB), ("xcrun", XCRUN_STUB), ("lipo", LIPO_STUB)):
            stub = self.bin / name
            stub.write_text(body, encoding="utf-8")
            stub.chmod(0o755)
        self.github_env = self.root / "github-env"
        self.github_output = self.root / "github-output"
        self.summary = self.root / "step-summary"
        for path in (self.github_env, self.github_output, self.summary):
            path.write_text("", encoding="utf-8")

    def run_step(self, job_id: str, step_name: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess:
        step = self.jobs[job_id].step(step_name)
        self.assertEqual(step.prop("shell"), "bash", step_name)
        script = step.run
        self.assertIsNotNone(script, step_name)
        self.assertNotIn("${{", script, f"{step_name} must take its inputs through env")
        script_path = self.root / "step.sh"
        script_path.write_text(script, encoding="utf-8")
        full_env = {
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            "HOME": str(self.root),
            "LC_ALL": "C",
            "STUB_STATE": str(self.state),
            "RUNNER_TEMP": str(self.runner_temp),
            "GITHUB_ENV": str(self.github_env),
            "GITHUB_OUTPUT": str(self.github_output),
            "GITHUB_STEP_SUMMARY": str(self.summary),
            "GITHUB_REPOSITORY": REPOSITORY,
            "GITHUB_SERVER_URL": "https://github.example",
            "GITHUB_RUN_ID": RUN_ID,
            "GITHUB_RUN_ATTEMPT": "2",
            "GITHUB_SHA": "a" * 40,
            "GH_TOKEN": "stub-token",
            **self._exported(self.github_env),
            **(env or {}),
        }
        return subprocess.run(
            ["bash", "--noprofile", "--norc", "-eo", "pipefail", str(script_path)],
            cwd=self.workspace,
            env=full_env,
            capture_output=True,
            text=True,
            check=False,
        )

    @staticmethod
    def _exported(path: Path) -> dict[str, str]:
        pairs = (line.split("=", 1) for line in path.read_text(encoding="utf-8").splitlines() if "=" in line)
        return {key: value for key, value in pairs}

    def outputs(self) -> dict[str, str]:
        return self._exported(self.github_output)

    def exported(self) -> dict[str, str]:
        return self._exported(self.github_env)

    def assert_refused(self, result: subprocess.CompletedProcess, reason: str) -> None:
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(reason, result.stdout + result.stderr)

    def add_artifact(self, artifact_id: str, source: Path, **kwargs: object) -> str:
        return _write_zip(self.state / f"artifact-{artifact_id}.zip", source, **kwargs)

    # -- fixtures ----------------------------------------------------------

    def make_payload(self, directory: Path) -> Path:
        directory.mkdir(parents=True)
        for name, content in (
            (UNIVERSAL, b"universal archive bytes\n"),
            ("maekon-linux-x64.tar.gz", b"linux archive bytes\n"),
            # GitHub publishes this `~` as `.`; see the read-back test.
            ("maekon-app_9.9.9~rc.1-1_amd64.deb", b"debian package bytes\n"),
            ("sbom.cdx.json", b'{"bomFormat":"CycloneDX"}\n'),
        ):
            path = directory / name
            path.write_bytes(content)
            path.with_name(name + ".sha256").write_text(f"{_sha256(path)}  dist/{name}\n", encoding="utf-8")
        return directory

    def install_signing_library(self) -> None:
        site = self.root / "site" / "nacl"
        site.mkdir(parents=True)
        (site / "__init__.py").write_text("", encoding="utf-8")
        (site / "signing.py").write_text(NACL_SIGNING_STUB, encoding="utf-8")
        python = self.runner_temp / "update-signing-venv" / "bin" / "python"
        python.parent.mkdir(parents=True)
        python.write_text(
            f'#!/usr/bin/env bash\nexport PYTHONPATH="{site.parent}"\nexec "{sys.executable}" "$@"\n',
            encoding="utf-8",
        )
        python.chmod(0o755)

    def write_trusted_keys(self, path: Path, keys: list[bytes]) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        entries = "".join(f'    "{_b64(key)}",\n' for key in keys)
        path.write_text(
            f"pub(crate) const TRUSTED_PUBLIC_KEYS: &[&str] = &[\n{entries}];\n",
            encoding="utf-8",
        )
        return path

    def build_final_set(self, *, seed: bytes = SIGNING_SEED, pinned: bytes = SIGNING_PUBLIC_KEY) -> subprocess.CompletedProcess:
        """Run the signing job's last three data steps on a fixture release."""
        self.install_signing_library()
        payload = self.make_payload(self.runner_temp / "release-payload")
        dist = self.workspace / "dist"
        dist.mkdir()
        (dist / DMG).write_bytes(b"notarized dmg bytes\n")
        (dist / PKG).write_bytes(b"notarized pkg bytes\n")
        recorded = self.run_step(
            SIGNING_JOB,
            "Record final notarized checksums",
            {"RELEASE_TAG": RELEASE_TAG, "SOURCE_RUN_ID": RUN_ID},
        )
        self.assertEqual(recorded.returncode, 0, recorded.stderr)
        assembled = self.run_step(SIGNING_JOB, "Assemble the final release set", {"PAYLOAD_DIR": str(payload)})
        self.assertEqual(assembled.returncode, 0, assembled.stderr)
        keys = self.write_trusted_keys(self.root / "trusted_keys.rs", [pinned])
        return self.run_step(
            SIGNING_JOB,
            "Sign release artifacts (Ed25519)",
            {"UPDATE_SIGNING_PRIVATE_KEY_B64": _b64(seed), "TRUSTED_KEYS_SOURCE": str(keys)},
        )

    def final_dist(self) -> Path:
        return self.runner_temp / "release-final" / "dist"


class SigningLibraryStandInTests(StepSandbox):
    def test_stand_in_is_real_ed25519(self) -> None:
        # If this drifted, the signing-step tests below would be exercising
        # something other than what PyNaCl computes.
        self.install_signing_library()
        python = self.runner_temp / "update-signing-venv" / "bin" / "python"
        program = (
            "import sys; from nacl.signing import SigningKey; "
            "key = SigningKey(bytes.fromhex(sys.argv[1])); "
            "print(bytes(key.verify_key).hex(), key.sign(bytes.fromhex(sys.argv[2])).signature.hex())"
        )
        for seed, public_key, message, signature in RFC8032_VECTORS:
            result = subprocess.run(
                [str(python), "-c", program, seed, message], capture_output=True, text=True, check=False
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.split(), [public_key, signature])
        derived = subprocess.run(
            [
                str(python),
                "-c",
                "import sys; from nacl.signing import SigningKey; "
                "print(bytes(SigningKey(bytes.fromhex(sys.argv[1])).verify_key).hex())",
                SIGNING_SEED.hex(),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(derived.returncode, 0, derived.stderr)
        self.assertEqual(derived.stdout.strip(), SIGNING_PUBLIC_KEY.hex())


class SmokeBindingTests(StepSandbox):
    STEP = "Bind the smoked macOS archive to the universal build"

    def setUp(self) -> None:
        super().setUp()
        assets = self.workspace / "smoke-assets"
        assets.mkdir()
        (assets / UNIVERSAL).write_bytes(b"universal archive bytes\n")
        self.digest = _sha256(assets / UNIVERSAL)

    def test_accepts_the_archive_the_universal_build_produced(self) -> None:
        result = self.run_step("release-reliability-smoke", self.STEP, {"EXPECTED_SHA256": self.digest})
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_refuses_a_different_archive(self) -> None:
        result = self.run_step("release-reliability-smoke", self.STEP, {"EXPECTED_SHA256": "0" * 64})
        self.assert_refused(result, "build-macos-universal produced")

    def test_refuses_when_the_build_reported_no_digest(self) -> None:
        result = self.run_step("release-reliability-smoke", self.STEP, {"EXPECTED_SHA256": ""})
        self.assert_refused(result, "did not report a SHA-256")


class PayloadAssemblyTests(StepSandbox):
    JOB = "assemble-release-payload"

    def setUp(self) -> None:
        super().setUp()
        self.dist = self.make_payload(self.workspace / "dist")
        for name in (
            "maekon-app-en-US-9.9.9.1-x86_64.msi",
            "Maekon_9.9.9-rc.1_x64-setup.exe",
            "maekon-macos-arm64.tar.gz",
            "maekon-macos-x64.tar.gz",
            "maekon-windows-x64.zip",
        ):
            (self.dist / name).write_bytes(name.encode())

    def test_complete_payload_is_covered(self) -> None:
        result = self.run_step(self.JOB, "Verify release payload coverage")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_file_is_named(self) -> None:
        for name in (UNIVERSAL, "maekon-windows-x64.zip", "maekon-app_9.9.9~rc.1-1_amd64.deb"):
            with self.subTest(name=name):
                held = (self.dist / name).read_bytes()
                (self.dist / name).unlink()
                result = self.run_step(self.JOB, "Verify release payload coverage")
                self.assert_refused(result, "Missing expected release payload file")
                (self.dist / name).write_bytes(held)

    def test_unsigned_flat_payload_is_accepted(self) -> None:
        result = self.run_step(self.JOB, "Refuse installers or signatures in the unsigned payload")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_payload_may_not_carry_what_the_signing_job_produces(self) -> None:
        for name in (DMG, PKG, f"{UNIVERSAL}.sig", MANIFEST, f"{MANIFEST}.sha256"):
            with self.subTest(name=name):
                (self.dist / name).write_bytes(b"x")
                result = self.run_step(self.JOB, "Refuse installers or signatures in the unsigned payload")
                self.assert_refused(result, "must be flat and must not carry")
                (self.dist / name).unlink()

    def test_payload_must_be_flat_regular_files(self) -> None:
        nested = self.dist / "nested"
        nested.mkdir()
        (nested / "file.bin").write_bytes(b"x")
        result = self.run_step(self.JOB, "Refuse installers or signatures in the unsigned payload")
        self.assert_refused(result, "nested")
        shutil.rmtree(nested)
        (self.dist / "link").symlink_to("/etc/hostname")
        result = self.run_step(self.JOB, "Refuse installers or signatures in the unsigned payload")
        self.assert_refused(result, "link")

    def test_already_published_tag_stops_before_the_signing_approval(self) -> None:
        env = {"RELEASE_TAG_NAME": RELEASE_TAG}
        result = self.run_step(self.JOB, "Refuse to rebuild an already published release", env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("No readable release", result.stdout)

        (self.state / "release-assets.txt").write_text("maekon-linux-x64.tar.gz\n", encoding="utf-8")
        result = self.run_step(self.JOB, "Refuse to rebuild an already published release", env)
        self.assertEqual(result.returncode, 0, result.stderr)

        (self.state / "release-assets.txt").write_text(f"{DMG}\n{MANIFEST}\n", encoding="utf-8")
        result = self.run_step(self.JOB, "Refuse to rebuild an already published release", env)
        self.assert_refused(result, "already published with notarized final bytes")

    def test_only_a_real_release_is_checked_against_published_state(self) -> None:
        step = self.jobs[self.JOB].step("Refuse to rebuild an already published release")
        self.assertEqual(step.prop("if"), "env.RELEASE_REHEARSAL != 'true'")


class PayloadHandoffTests(StepSandbox):
    STEP = "Download the unsigned release payload"

    def setUp(self) -> None:
        super().setUp()
        self.payload = self.make_payload(self.root / "payload-source")
        self.env = {
            "PAYLOAD_ARTIFACT_ID": "777",
            "PAYLOAD_ARTIFACT_DIGEST": self.add_artifact("777", self.payload),
            "UNIVERSAL_SHA256": _sha256(self.payload / UNIVERSAL),
        }

    def test_payload_arrives_intact(self) -> None:
        result = self.run_step(SIGNING_JOB, self.STEP, self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        payload_dir = Path(self.exported()["PAYLOAD_DIR"])
        self.assertEqual(
            sorted(path.name for path in payload_dir.iterdir()),
            sorted(path.name for path in self.payload.iterdir()),
        )
        self.assertIn(f"repos/{REPOSITORY}/actions/artifacts/777/zip", (self.state / "gh.log").read_text())

    def test_digest_may_carry_the_api_prefix(self) -> None:
        env = {**self.env, "PAYLOAD_ARTIFACT_DIGEST": "sha256:" + self.env["PAYLOAD_ARTIFACT_DIGEST"]}
        self.assertEqual(self.run_step(SIGNING_JOB, self.STEP, env).returncode, 0)

    def test_refuses_a_payload_other_than_the_one_uploaded(self) -> None:
        result = self.run_step(SIGNING_JOB, self.STEP, {**self.env, "PAYLOAD_ARTIFACT_DIGEST": "0" * 64})
        self.assert_refused(result, "but assemble-release-payload uploaded")
        self.assertNotIn("PAYLOAD_DIR", self.exported())

    def test_refuses_missing_or_malformed_coordinates(self) -> None:
        for key, value, reason in (
            ("PAYLOAD_ARTIFACT_ID", "", "did not report an artifact id and digest"),
            ("PAYLOAD_ARTIFACT_ID", "777 --paginate", "did not report an artifact id and digest"),
            ("PAYLOAD_ARTIFACT_DIGEST", "", "did not report an artifact id and digest"),
            ("UNIVERSAL_SHA256", "", "did not report a SHA-256"),
        ):
            with self.subTest(key=key, value=value):
                self.assert_refused(self.run_step(SIGNING_JOB, self.STEP, {**self.env, key: value}), reason)
                self.assertNotIn("PAYLOAD_DIR", self.exported())

    def test_refuses_a_payload_that_already_carries_signatures_or_installers(self) -> None:
        for name in (f"{UNIVERSAL}.sig", DMG, PKG, MANIFEST):
            with self.subTest(name=name):
                (self.payload / name).write_bytes(b"x")
                env = {**self.env, "PAYLOAD_ARTIFACT_DIGEST": self.add_artifact("777", self.payload)}
                self.assert_refused(self.run_step(SIGNING_JOB, self.STEP, env), "must be flat and unsigned")
                self.assertNotIn("PAYLOAD_DIR", self.exported())
                (self.payload / name).unlink()

    def test_refuses_a_payload_that_is_not_flat_regular_files(self) -> None:
        (self.payload / "nested").mkdir()
        (self.payload / "nested" / "file.bin").write_bytes(b"x")
        env = {**self.env, "PAYLOAD_ARTIFACT_DIGEST": self.add_artifact("777", self.payload)}
        self.assert_refused(self.run_step(SIGNING_JOB, self.STEP, env), "must be flat and unsigned")
        shutil.rmtree(self.payload / "nested")

        digest = self.add_artifact("777", self.payload, extra=[_symlink_entry("link")])
        result = self.run_step(SIGNING_JOB, self.STEP, {**self.env, "PAYLOAD_ARTIFACT_DIGEST": digest})
        self.assert_refused(result, "must be flat and unsigned")
        self.assertNotIn("PAYLOAD_DIR", self.exported())

    def test_refuses_binaries_other_than_the_smoke_tested_ones(self) -> None:
        result = self.run_step(SIGNING_JOB, self.STEP, {**self.env, "UNIVERSAL_SHA256": "0" * 64})
        self.assert_refused(result, "but the smoke-tested build is")
        self.assertNotIn("PAYLOAD_DIR", self.exported())


class UniversalBinaryTests(StepSandbox):
    STEP = "Extract the universal binaries"

    def make_archive(self, names: tuple[str, ...]) -> dict[str, str]:
        payload = self.runner_temp / "release-payload"
        staging = self.root / "staging"
        payload.mkdir(exist_ok=True)
        shutil.rmtree(staging, ignore_errors=True)
        staging.mkdir()
        for name in names:
            (staging / name).write_bytes(name.encode())
        subprocess.run(["tar", "-czf", str(payload / UNIVERSAL), "-C", str(staging), *names], check=True)
        return {"PAYLOAD_DIR": str(payload)}

    def test_extracts_both_universal_binaries(self) -> None:
        env = self.make_archive(("maekon", "maekon-sandbox-worker", "icon.icns"))
        result = self.run_step(SIGNING_JOB, self.STEP, env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.workspace / "binaries" / "maekon-sandbox-worker").is_file())

    def test_refuses_an_archive_without_the_sidecar(self) -> None:
        env = self.make_archive(("maekon", "icon.icns"))
        self.assert_refused(self.run_step(SIGNING_JOB, self.STEP, env), "no regular file named maekon-sandbox-worker")

    def test_refuses_a_single_architecture_binary(self) -> None:
        env = self.make_archive(("maekon", "maekon-sandbox-worker", "icon.icns"))
        result = self.run_step(SIGNING_JOB, self.STEP, {**env, "LIPO_ARCHS": "arm64"})
        self.assert_refused(result, "is not a universal arm64 + x86_64 binary")


class FinalSetSigningTests(StepSandbox):
    def verify(self, dist: Path, keys: Path) -> subprocess.CompletedProcess:
        return subprocess.run(
            [
                sys.executable,
                str(VERIFIER),
                "--dist",
                str(dist),
                "--trusted-keys",
                str(keys),
                "--release-tag",
                RELEASE_TAG,
                "--source-run-id",
                RUN_ID,
            ],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_signed_set_passes_the_publication_gate(self) -> None:
        signed = self.build_final_set()
        self.assertEqual(signed.returncode, 0, signed.stderr)
        self.assertIn("Signed artifacts: 7", signed.stdout)
        dist = self.final_dist()
        names = sorted(path.name for path in dist.iterdir())
        assets = [name for name in names if not name.endswith((".sha256", ".sig"))]
        self.assertEqual(len(assets), 7)
        self.assertEqual(len(names), 21)
        # Sidecar text stays what releases have always published: installers
        # and manifest name the bare file, everything else names `dist/<file>`.
        for name in (DMG, PKG, MANIFEST):
            self.assertEqual((dist / f"{name}.sha256").read_text(), f"{_sha256(dist / name)}  {name}\n")
        self.assertEqual(
            (dist / f"{UNIVERSAL}.sha256").read_text(),
            f"{_sha256(dist / UNIVERSAL)}  dist/{UNIVERSAL}\n",
        )
        verified = self.verify(dist, self.root / "trusted_keys.rs")
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertIn("release final bytes verified: 7 assets, 21 files", verified.stdout)

    def test_refuses_to_sign_with_a_key_the_client_does_not_pin(self) -> None:
        signed = self.build_final_set(seed=UNPINNED_SEED)
        self.assert_refused(signed, "does not match any key pinned")
        self.assertEqual(list(self.final_dist().glob("*.sig")), [])

    def test_refuses_to_sign_a_file_without_a_checksum(self) -> None:
        self.install_signing_library()
        payload = self.make_payload(self.runner_temp / "release-payload")
        (payload / "sbom.cdx.json.sha256").unlink()
        dist = self.workspace / "dist"
        dist.mkdir()
        (dist / DMG).write_bytes(b"dmg")
        (dist / PKG).write_bytes(b"pkg")
        self.run_step(SIGNING_JOB, "Record final notarized checksums", {"RELEASE_TAG": RELEASE_TAG, "SOURCE_RUN_ID": RUN_ID})
        self.run_step(SIGNING_JOB, "Assemble the final release set", {"PAYLOAD_DIR": str(payload)})
        keys = self.write_trusted_keys(self.root / "trusted_keys.rs", [SIGNING_PUBLIC_KEY])
        signed = self.run_step(
            SIGNING_JOB,
            "Sign release artifacts (Ed25519)",
            {"UPDATE_SIGNING_PRIVATE_KEY_B64": _b64(SIGNING_SEED), "TRUSTED_KEYS_SOURCE": str(keys)},
        )
        self.assert_refused(signed, "Refusing to sign sbom.cdx.json")

    def test_manifest_names_this_tag_and_this_run(self) -> None:
        self.assertEqual(self.build_final_set().returncode, 0)
        step = self.jobs[SIGNING_JOB].step("Record final notarized checksums")
        self.assertIn("RELEASE_TAG: ${{ needs.verify-version.outputs.release_tag }}", step.text)
        self.assertIn("SOURCE_RUN_ID: ${{ github.run_id }}", step.text)
        wrong_run = subprocess.run(
            [sys.executable, str(VERIFIER), "--dist", str(self.final_dist()), "--trusted-keys",
             str(self.root / "trusted_keys.rs"), "--release-tag", RELEASE_TAG, "--source-run-id", "1"],
            capture_output=True, text=True, check=False,
        )
        self.assertNotEqual(wrong_run.returncode, 0)


class PublicationTests(StepSandbox):
    def setUp(self) -> None:
        super().setUp()
        self.assertEqual(self.build_final_set().returncode, 0)
        final = self.final_dist()
        self.env = {
            "FINAL_ARTIFACT_ID": "900",
            "FINAL_ARTIFACT_DIGEST": self.add_artifact("900", final),
            "RELEASE_TAG_NAME": RELEASE_TAG,
            "SMOKED_DMG_SHA256": _sha256(final / DMG),
            "SMOKED_PKG_SHA256": _sha256(final / PKG),
        }
        # A fresh runner: only the two checkouts the publish job makes.
        shutil.rmtree(self.workspace)
        self.workspace.mkdir()
        tools = self.workspace / ".workflow-tools" / "scripts"
        tools.mkdir(parents=True)
        shutil.copy2(VERIFIER, tools / VERIFIER.name)
        self.write_trusted_keys(
            self.workspace / "src-tauri" / "src" / "updater" / "trusted_keys.rs", [SIGNING_PUBLIC_KEY]
        )
        self.github_env.write_text("", encoding="utf-8")

    def download(self, **overrides: str) -> subprocess.CompletedProcess:
        return self.run_step(PUBLISH_JOB, "Download the signed final release set", {**self.env, **overrides})

    def gate(self, **overrides: str) -> subprocess.CompletedProcess:
        return self.run_step(
            PUBLISH_JOB, "Verify final bytes against the pinned update keys", {**self.env, **overrides}
        )

    def publish(self) -> Path:
        """What GitHub serves after `gh release create dist/*`."""
        published = self.state / "published"
        published.mkdir()
        for path in (self.workspace / "dist").iterdir():
            shutil.copy2(path, published / path.name.replace("~", "."))
        (self.state / "release-id").write_text("555\n", encoding="utf-8")
        return published

    def readback(self) -> subprocess.CompletedProcess:
        return self.run_step(PUBLISH_JOB, "Verify published release bytes", self.env)

    def deleted(self) -> list[str]:
        log = self.state / "deleted.log"
        return log.read_text(encoding="utf-8").splitlines() if log.is_file() else []

    def test_final_set_reaches_the_gate_and_passes(self) -> None:
        self.assertEqual(self.download().returncode, 0)
        self.assertEqual(len(list((self.workspace / "dist").iterdir())), 21)
        result = self.gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("release final bytes verified: 7 assets, 21 files, pinned update key #0", result.stdout)

    def test_refuses_a_final_set_other_than_the_one_signed(self) -> None:
        self.assert_refused(self.download(FINAL_ARTIFACT_DIGEST="0" * 64), "but sign-notarize-macos uploaded")
        self.assertFalse((self.workspace / "dist").exists())
        self.assert_refused(self.download(FINAL_ARTIFACT_ID=""), "did not report an artifact id and digest")

    def test_gate_refuses_installers_the_smoke_did_not_launch(self) -> None:
        self.assertEqual(self.download().returncode, 0)
        self.assert_refused(self.gate(SMOKED_DMG_SHA256="0" * 64), "but the bytes accepted earlier in this run were")
        self.assert_refused(self.gate(SMOKED_PKG_SHA256=""), "--expect-sha256 needs NAME=<64 lowercase hex>")

    def test_gate_refuses_bytes_changed_after_signing(self) -> None:
        self.assertEqual(self.download().returncode, 0)
        target = self.workspace / "dist" / "maekon-linux-x64.tar.gz"
        target.write_bytes(target.read_bytes() + b"x")
        self.assert_refused(self.gate(), "stale checksum sidecar for maekon-linux-x64.tar.gz")
        target.with_name(target.name + ".sha256").write_text(f"{_sha256(target)}  dist/{target.name}\n")
        self.assert_refused(self.gate(), "stale signature sidecar for maekon-linux-x64.tar.gz")

    def test_gate_refuses_a_set_signed_by_an_unpinned_key(self) -> None:
        self.assertEqual(self.download().returncode, 0)
        self.write_trusted_keys(
            self.workspace / "src-tauri" / "src" / "updater" / "trusted_keys.rs",
            [bytes.fromhex("fefe2ce26bbffd7e311578c5fe66d7a86a0084a326168e0d353401e5b4b68d9e")],
        )
        self.assert_refused(self.gate(), "does not verify under any pinned update key")

    def test_published_release_matches_and_is_kept(self) -> None:
        self.assertEqual(self.download().returncode, 0)
        published = self.publish()
        # The fixture reproduces the rename GitHub applies to the DEB.
        self.assertTrue((published / "maekon-app_9.9.9.rc.1-1_amd64.deb").is_file())
        result = self.readback()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.deleted(), [])

    def test_published_release_with_changed_bytes_is_deleted(self) -> None:
        self.assertEqual(self.download().returncode, 0)
        published = self.publish()
        target = published / "sbom.cdx.json"
        target.write_bytes(target.read_bytes() + b" ")
        result = self.readback()
        self.assert_refused(result, f"Deleted invalid release for {RELEASE_TAG}")
        self.assertEqual(self.deleted(), [f"repos/{REPOSITORY}/releases/555"])

    def test_published_release_missing_an_asset_is_deleted(self) -> None:
        self.assertEqual(self.download().returncode, 0)
        published = self.publish()
        for suffix in ("", ".sha256", ".sig"):
            (published / f"sbom.cdx.json{suffix}").unlink()
        result = self.readback()
        self.assert_refused(result, "Published assets are not the verified release set")
        self.assertEqual(self.deleted(), [f"repos/{REPOSITORY}/releases/555"])

    def test_published_release_with_an_extra_asset_is_deleted(self) -> None:
        self.assertEqual(self.download().returncode, 0)
        published = self.publish()
        (published / "unreviewed.bin").write_bytes(b"x")
        self.assert_refused(self.readback(), f"Deleted invalid release for {RELEASE_TAG}")
        self.assertEqual(self.deleted(), [f"repos/{REPOSITORY}/releases/555"])

    def test_release_that_cannot_be_read_back_is_deleted(self) -> None:
        self.assertEqual(self.download().returncode, 0)
        (self.state / "release-id").write_text("555\n", encoding="utf-8")
        self.assert_refused(self.readback(), f"Deleted invalid release for {RELEASE_TAG}")
        self.assertEqual(self.deleted(), [f"repos/{REPOSITORY}/releases/555"])


class InstallerSmokeHandoffTests(StepSandbox):
    STEP = "Download the final macOS installers"

    def setUp(self) -> None:
        super().setUp()
        self.env: dict[str, str] = {}
        self.digests: dict[str, str] = {}
        for key, artifact_id, name in (("DMG", "801", DMG), ("PKG", "802", PKG)):
            source = self.root / f"upload-{key}"
            source.mkdir()
            (source / name).write_bytes(f"final {name} bytes\n".encode())
            self.digests[name] = _sha256(source / name)
            self.env[f"{key}_ARTIFACT_ID"] = artifact_id
            self.env[f"{key}_ARTIFACT_DIGEST"] = self.add_artifact(artifact_id, source)

    def test_reports_the_digests_of_the_bytes_it_will_launch(self) -> None:
        result = self.run_step(INSTALLER_SMOKE_JOB, self.STEP, self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.outputs(), {"dmg_sha256": self.digests[DMG], "pkg_sha256": self.digests[PKG]})
        self.assertEqual(
            (self.state / "xcrun.log").read_text().splitlines(),
            [f"stapler validate smoke-installers/{DMG}", f"stapler validate smoke-installers/{PKG}"],
        )

    def test_refuses_an_installer_other_than_the_one_signed(self) -> None:
        result = self.run_step(INSTALLER_SMOKE_JOB, self.STEP, {**self.env, "PKG_ARTIFACT_DIGEST": "0" * 64})
        self.assert_refused(result, "but sign-notarize-macos uploaded")
        self.assertEqual(self.outputs(), {})

    def test_refuses_an_installer_without_a_notarization_ticket(self) -> None:
        result = self.run_step(INSTALLER_SMOKE_JOB, self.STEP, {**self.env, "XCRUN_UNSTAPLED": PKG})
        self.assert_refused(result, "does not have a ticket stapled")
        self.assertEqual(self.outputs(), {})

    def test_refuses_missing_coordinates(self) -> None:
        result = self.run_step(INSTALLER_SMOKE_JOB, self.STEP, {**self.env, "DMG_ARTIFACT_ID": ""})
        self.assert_refused(result, "did not report an artifact id and digest")
        self.assertEqual(self.outputs(), {})


class FailureReporterTests(StepSandbox):
    def test_reporter_names_the_run_and_attempt_and_says_nothing_shipped(self) -> None:
        job = self.jobs[SIGNING_JOB]
        reporter = next(step for step in job.steps if step.prop("if") == "failure()")
        result = self.run_step(SIGNING_JOB, reporter.name)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            f"MAEKON_NOTARIZATION_FAILURE_V1 workflow=release.yml run_id={RUN_ID} attempt=2 source_sha={'a' * 40}",
            result.stdout,
        )
        summary = self.summary.read_text(encoding="utf-8")
        self.assertIn(f"/actions/runs/{RUN_ID}/attempts/2", summary)
        self.assertIn("Nothing was published", summary)
        self.assertIn("private ONESHIM", summary)


if __name__ == "__main__":
    unittest.main()
