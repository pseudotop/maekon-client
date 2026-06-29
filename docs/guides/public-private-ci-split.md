# Public and Private CI Split

This guide defines the CI boundary for the Maekon Client hybrid contribution
lane. It is intentionally narrow until external contributor PR volume justifies
stronger automation.

For the contributor-facing PR route around these checks, see
[`public-contributor-path.md`](./public-contributor-path.md).

## Goals

- Public PR authors get fast, actionable feedback from fork-safe checks.
- Maintainers keep release, signing, capture, privacy, and security validation
  behind maintainer-controlled gates.
- Fork PRs never receive repository secrets, signing keys, private validation
  credentials, or internal environment variables.
- Sensitive validation can be summarized publicly without exposing raw evidence
  or maintainer-only infrastructure details.

## Public Synthetic Matrix

These checks are safe for ordinary public pull requests because they use source
files, synthetic fixtures, generated stubs, public dependency metadata, SBOMs,
or headless browsers only.

| Check | Workflow | Data allowed | Required during Phase 0/1 |
| --- | --- | --- | --- |
| Frontend build and E2E | `.github/workflows/ci.yml` | Synthetic frontend fixtures and Playwright artifacts | Yes, when check names are stable |
| Rust fmt/clippy/check/test | `.github/workflows/ci.yml` | Repository source and generated local stubs | Yes, when check names are stable |
| Config sync | `.github/workflows/config-sync.yml` | Static config files and generated frontend stub | Yes |
| gRPC governance | `.github/workflows/grpc-governance.yml` | Public proto files and generated code | Yes |
| Public export guardrails | `.github/workflows/ci.yml` and parent validation | Exported source tree only | Blocking for public CI checks that run in `ci.yml`; parent validation remains maintainer-controlled evidence |
| Supply-chain and integrity checks | `.github/workflows/security-compliance.yml` on PRs, pushes to `main`, and manual dispatch | Public dependency metadata, SBOM, and generated reports | Blocking for the exported public supply-chain gate |

`security-compliance.yml` is the authoritative public supply-chain gate. It
runs RustSec audit, cargo-deny licenses/advisories/sources/bans,
exemption-expiry validation, cargo-vet, third-party notice generation, and SBOM
generation. Do not treat a red security-compliance check as advisory.

Public synthetic checks must not require real screen capture, microphone input,
browser session state, OS permission dialogs, signing credentials, release
tokens, or external provider credentials.

## Required vs Advisory Checks

After Phase 0/1 stabilization, public branch protection should require only
checks that are fork-safe, stable by name, and useful to contributors.

| Check class | Phase 0/1 posture | Phase 2 posture |
| --- | --- | --- |
| Config Sync / Port & Version Sync | Required | Required |
| gRPC Governance / Contract and Readiness Gate | Required | Required |
| Security & Compliance / Supply Chain Controls | Required | Required |
| CI / Rust fmt, clippy, check, tests, and build targets | Required once check names are stable | Required |
| CI / Frontend build and E2E | Required once check names are stable | Required for public UI/docs surfaces that exercise frontend assets |
| CodeQL | Required when enabled with stable check names; otherwise advisory until stable | Required when enabled |
| Public export guardrails that run in public CI | Required | Required |
| Performance gates and budget checks | Advisory unless published budgets and low flake rate exist | Required only after budgets, owners, and failure handling are documented |
| Parent validation and maintainer-only trust-core gates | Label/review gated, not public required checks | Still not public required checks |
| Release signing, notarization, and installer provenance | Required for tag/release environments, not fork PRs | Same |

Do not promote a check to required while its name is still drifting, its failure
message requires maintainer-only context, or it needs private data to diagnose.
Promote checks one at a time after maintainers can explain the failure and
remediation path in a public PR without private evidence.

## Private Gate Triggers

Maintainers decide when to run maintainer-controlled gates. Use the public labels
from `docs/guides/public-contribution-governance.md` to explain the route.

| Trigger | Meaning |
| --- | --- |
| `ok-to-test` | A maintainer has reviewed the public PR enough to run maintainer-controlled tests |
| `security-reviewed` | Security/privacy review has cleared the public handling path |
| `do-not-merge/private-test` | Parent import or release waits for maintainer-only validation |
| `do-not-merge/security` | Security handling is still active; do not discuss details publicly |
| `imported-to-parent` | The public patch has been imported into the parent source tree for full validation |

Private gates cover real OS permissions, sandbox behavior, automation policy,
installer/update flows, release signing, and adversarial privacy checks. Public
comments should summarize only the safe outcome, such as:

> Maintainer-only privacy validation passed for the relevant risk class. No
> sensitive evidence is included in this public thread.

The minimum public-safe trust-core report is:

- the lane and risk class;
- whether maintainer-only validation was required;
- a pass/fail/blocked outcome;
- a public parent PR, public export, or release reference when one exists;
- a short remediation or follow-up pointer if blocked.

Do not include private test names, private logs, raw captures, screenshots,
local absolute paths, maintainer-only infrastructure names, secret identifiers,
or unpublished roadmap details in that report.

## Fork PR Secret Policy

Public workflows must follow these rules:

1. Do not use `pull_request_target`.
2. Do not reference `secrets.*` from a workflow that runs on `pull_request`.
3. Do not request write permissions from a workflow that runs on `pull_request`.
4. Keep top-level workflow permissions explicit and read-only for PR workflows.
5. Keep release, signing, and deployment workflows on `workflow_dispatch`, tag,
   or maintainer-controlled events.

The guardrail script `scripts/ci/check-public-private-ci-split.sh` enforces the
first four rules for exported public workflows.

## Branch Protection

During Phase 0/1, required checks should be limited to stable public checks in
the table above. Maintainer-only gates should be represented by labels and
public review summaries, not by public required checks that expose sensitive
names or evidence.

When the public repository starts receiving regular external PRs, maintainers
can promote additional stable public checks to required checks one at a time.

For the maintainer handoff after a public patch is imported into the parent
source tree, use [`hybrid-import-workflow.md`](./hybrid-import-workflow.md).
