[English](./ADR-035-release-pipeline-determinism.md) | [한국어](./ADR-035-release-pipeline-determinism.ko.md)

# ADR-035: The release pipeline decides in one place, and the public repository is never where we learn something is broken

**Status**: Accepted — 2026-08-24
**Date**: 2026-08-21
**Scope**: release checklist and its gates; release-decision evidence; toolchain determinism; the export that regenerates the public repository; the tag-to-assets path; the handoff to the updater — reversal, rollout stage, credential lifetime
**Related**: ADR-005 (Tauri governance), `docs/guides/public-private-ci-split.md`, `docs/guides/hybrid-import-workflow.md`, `docs/release-checklist.md`, `docs/contracts/release-decision-manifest.md`
**Issue**: #11330

---

## Implementation record

Acceptance fixes the architectural direction; it does not relabel unfinished
migration work as release evidence. The current source was re-measured on
2026-08-24:

| Decision | State | Current enforcement or remaining boundary |
| --- | --- | --- |
| D1 verification parity | Partial | The stable canary mechanically matches the three public clippy invocations. Full parent-PR counterparts for all 15 public required checks are not yet present. |
| D2 identity and freshness | Implemented | The manifest binds commit SHA and artifact checksum, and its authorization clock starts at manifest build. The contract documents why the one-hour decision window remains. |
| D3 checklist dispositions | Implemented | #11467 / PR #11472 assign all **69 current** stable checklist IDs a machine, evidence, or human disposition, including the post-publish updater item. |
| D4 deterministic toolchain | Partial | #11324 added MSRV and scheduled stable-canary coverage. The exact build-toolchain pin remains owned by #11299. |
| D5 export promotion | Implemented | `update-public-repo-clone.sh` continues an existing export branch, binds provenance to its actual parent, and refuses public-base or branch divergence. Its regression test includes both descendant and injected-divergence controls. |
| D6 single release entry point | Partial | #11256 added the signing ability probe and signed-tag path. Manifest production is still a separate operator input, so the final composition remains future migration work. |
| D7 post-publish updater lifecycle | Implemented | The 69-item registry keeps updater detection pending before publication and the independent post-publish receipt binds the observation to the exact tag and commit. |

Rows marked partial are not evidence that the corresponding release property is
complete. They preserve the migration boundary without keeping this decision in
Draft indefinitely.

---

## Context

`v0.0.1-rc.9` shipped on 2026-08-21 with 42 assets and macOS notarization. Getting
there took most of a day, and almost none of that day was spent releasing.

Five things went wrong. They are worth separating, because only two of them were
defects in the product.

**1 — A tag was pushed unsigned and the release produced nothing.** `release.yml`
requires the tag signature to be GitHub-verified, and that check can only run
once the tag exists — by which point the tag is pushed. The pre-release gate had
reported `0 errors — OK to proceed` minutes earlier. Published assets: 0. Same
shape as the earlier release that shipped no assets.

**2 — A passing smoke was flipped to failing by its own cleanup.** The macOS
installer smoke printed `macOS installer smoke completed`, then its EXIT trap ran
`rm -rf` over a DMG that was still mounted. Every path returned
`Read-only file system`, the `rm` exited non-zero, and because it was the last
statement of the trap it became the script's status. `create-release` needs that
job, so a build that had passed every check could not publish.

**3 — An upstream toolchain release blocked every public pull request.** Rust
1.98.0 became stable and its new `clippy::chunks_exact_to_as_chunks` lint fired
under `-D warnings`. Nothing in the repository had changed; the same source
passed on 1.97.1 the day before. `Check (fmt + clippy)` is a required status
check, so every open pull request — including export snapshots carrying no Rust
changes at all — became unmergeable.

**4 — Clearing that lint took four export round trips.** Clippy stops at the
first failing target, so each export revealed exactly one more site. The cost per
attempt was not a compile: it was *parent pull request → merge → regenerate
export → public pull request → approval*. The loop only ended when the same
clippy invocation was finally run locally and reported the whole remaining set at
once.

**5 — The evidence window raced a paid job.** The release-decision manifest
expires 3600 seconds after the benchmark report's `generated_at`. The desktop
smoke that produces that report runs about 36 minutes. Everything after it —
manifest build, gate, signing, tagging — had to fit in the remainder. The
contract document does not say why the window is one hour; it says only
"configured freshness window".

### What those five have in common

Four of them are not product defects. They are consequences of **where the
decision is made**.

| Measured | Value |
| --- | --- |
| Manual checkboxes in the release checklist | **68** across 7 sections |
| Required status checks on public `main` | **15**, strict, no merge queue |
| Release-decision freshness window | 3600s, **no documented rationale** |
| Places installing Rust targets | **10**, one of them in `release.yml` |
| Verification that exists only publicly | `Check (fmt + clippy)` and the rest of the required set |

The pipeline is not short of gates. It has too many places that can only answer
*after* an irreversible or expensive step: the tag must exist before its
signature is judged; the export must be published before the required checks
speak; the paid smoke must finish before the clock that constrains it starts.

Two further findings are structural rather than incidental:

- Guards existed that nothing executed. On 2026-08-21 the macOS installer
  preserve test had **zero callers**; it has since been wired into the export
  guardrail chain, so a reader checking today will find one. The only workflow
  running `cargo clippy` on this workspace is dispatch-only, so it has never run
  on its own — that one is still open.

  Every number in the table above is a measurement taken on 2026-08-21, and some
  are already being changed by work this ADR prompted. They are recorded as the
  evidence for the decision, not as a claim about the present.
- A gate that reports `OK to proceed` while a required property is unmeasured is
  worse than a missing gate: it reads as evidence.

At the intended cadence — **weekly or faster** — a 68-item manual checklist is
not maintainable, and a four-round-trip fix loop is not survivable.

## Decision

**Every property that can block a release is decided before the step that makes
it expensive to fix, and the public repository only ever receives trees that have
already passed.**

Six commitments follow.

### D1. Verification parity, decided internally

Every required status check on public `main` has a counterpart that runs on the
parent pull request, over the same inputs and with the same invocation. The
export carries only trees that already passed it.

Parity is asserted mechanically, not by convention: a guard extracts the
invocation set from both sides, normalizes it, and fails when they differ. A
canary that lints a different feature set than the required check would report
green while the gate that blocks merges is red.

**Parity covers the toolchain, not only the invocation.** The same command on two
different compilers is two different checks — that is precisely how failure 3
happened. So the parity guard compares the resolved toolchain as well, and D4's
pin must land on both sides or neither. A state where one side is pinned and the
other floats makes D1 quietly false while every guard still reports green.

This is what removes the round trips in failure 4.

### D2. Evidence binds to identity first, and to a window sized for the pipeline

Release-decision evidence must demonstrably describe **this commit and this
artifact** — commit SHA plus artifact checksum. Identity is the primary binding
and it is the one the current contract under-uses.

Identity alone is not sufficient, and an earlier draft of this ADR was wrong to
say so. Smoke evidence is **environment-dependent**: the same commit can pass on
one runner image and fail on the next, a dependency can be yanked, an advisory
can land. Evidence kept forever would let a release ship on a months-old
observation of a commit whose world has since changed. That decay is the real
argument for a freshness window, and it survives identity binding.

What does not survive is a window **sized without reference to the pipeline it
constrains**. The current 3600s is measured from the benchmark report's
`generated_at`, and the job producing that report takes about 36 minutes — so
manifest build, gate, signing and tagging share whatever is left. The contract
does not state why one hour, only "configured freshness window".

So: keep a window, state the decay it models, and measure it from the moment the
**decision** is taken rather than from the observation that feeds it. A window
that cannot accommodate its own pipeline is not protecting the release; it is
hurrying the operator.

"The moment the decision is taken" must not be read as *whenever the single
entry point (D6) runs* — that would make this step wait for the last one in the
migration. The decision moment is **manifest build**, which exists today and is
already the point where evidence, commit, and artifact are bound together. D6
later wraps that moment in one command; it does not create it.

### D3. The checklist stays; the checking does not stay manual

All 68 items remain. Each one carries exactly one disposition:

| Disposition | Meaning |
| --- | --- |
| `machine` | A command asserts it and emits evidence. Failure blocks. |
| `evidence` | A command collects the artifact; a human reads and signs. |
| `human` | Judgment that cannot be mechanized. The registry records **why**. |

A registry maps item → disposition → command. A guard fails when an item has no
disposition, so new checklist entries cannot arrive as prose. `human` is a
legitimate answer; an unexamined item is not.

**The items need stable identifiers before a registry can key off them.** Several
wrap across lines today, so extracting them by text yields fragments — one comes
out as the single word `Any`. Prose is not a key: it changes when someone
rewords, and a registry keyed on it would silently lose its mapping. Giving each
item an ID is therefore the first move of this step, not an afterthought.

The pass must also encode **when a subject can be observed**. D7 contains an
item whose runtime evidence cannot exist until publication. Marking it `human`
or pretending a mock test is the observation would leave an operator ticking
something unverifiable. The registry therefore distinguishes pre-publish and
post-publish phases while keeping both in one visible checklist.

Several items *are* the required checks — "Quick suite (PR CI) — all green" is
one. Those are owned by D1, and the registry **references the lane rather than
restating its command**. Two places holding the same invocation is how the
canary/required-check pair would have drifted, and it is the failure this ADR
puts a parity guard against; a disposition registry that copies commands would
reintroduce it one layer up.

### D4. The toolchain is deterministic, and drift is discovered on our schedule

The build toolchain is pinned. Because ten places install Rust targets against
the floating channel, pinning is its own change that updates all of them
together — an exact pin sends cargo to a different toolchain entry that has none
of those targets.

Pinning alone would hide new lints until an upgrade. A scheduled canary
therefore runs the newest stable against the same invocations as the required
check, so a new release's lints arrive as a report on a chosen day rather than
as a frozen repository mid-export.

### D5. Export is a promotion, and it refuses to overwrite the public repository

Two changes:

- The export branch is **fast-forwardable**. Today each regeneration starts from
  the base branch, so a corrected snapshot is a sibling rather than a descendant
  and cannot update the open pull request. One fix therefore costs a new branch,
  a new pull request, and a new approval.
- The export **refuses to run when the public branch has moved since the snapshot
  it last recorded**. Public contributions are imported into the parent and then
  reappear in the next regeneration
  (`docs/guides/hybrid-import-workflow.md`). Until that import lands, a
  regeneration silently reverts the contribution. Today the ordering is a
  documented discipline with nothing enforcing it.

  The comparison cannot be "commits not present in the parent": the two
  repositories do not share history, so their SHAs never correspond. The export
  provenance already records what is needed — `ssot.source_sha` for the parent
  commit and `public_target.base_sha` for the public commit the snapshot was
  built on. Divergence is therefore *public HEAD has advanced past the recorded
  `base_sha` by something that is not our own export commit*, and that is what
  the refusal tests.

### D6. One entry point from decision to assets

Releasing is a single command that runs the gate, produces and validates the
evidence, proves the signing key can sign, creates the signed tag, and reports.
No step depends on an operator remembering an order.

The signing check is the pattern for the whole path: the signature cannot be
verified before the tag exists, but the **ability to produce one** can be — by
signing throwaway bytes rather than by reading configuration. `user.signingkey`
being set says nothing about whether the key is present, readable, or unlocked.

### D7. The updater exists, but its observation belongs after publication

The first measurement in this ADR looked only at Tauri's plugin configuration
and concluded that no updater was wired. That conclusion was incomplete. Maekon
ships a custom updater in `src-tauri/src/updater`: stable clients query GitHub's
`/releases/latest`, while prerelease clients query `/releases?per_page=1` and
select signed platform assets. Mock API tests cover both channels and an ignored
network test reaches the real GitHub Releases API.

The empty `plugins.updater` endpoint and the absence of a Tauri update manifest
therefore do not prove updater absence. They prove only that Maekon does not use
the Tauri updater plugin as its delivery adapter.

The remaining defect is lifecycle ordering. The release-decision manifest is a
pre-tag authorization gate, but *"a previous RC detects the new version"* is
observable only after the signed tag produces a public GitHub Release. Requiring
that observation to pass before the tag creates an impossible cycle; marking it
passed from mock tests would confuse implementation proof with runtime evidence.

So this decision states three things:

- **The pipeline owns both phases.** Pre-publish authorization records the
  updater observation as explicit `pending`; post-publish completion requires a
  previous-RC runtime receipt bound to the exact tag and commit.
- **Reversal is rehearsed before it is needed.** The recovery most recently
  exercised was manual and undocumented: confirm no release object exists, delete
  the tag, recreate it. It worked only because the failure landed *before* assets
  published. `docs/guides/updater-rollback-windows.md` describes a path nobody
  has walked.
- **Checklist items may not outrun their lifecycle.** A post-publish item may be
  pending before publication, but it cannot disappear, become a mock-test pass,
  or be used to call the RC operationally complete.

This correction does not redesign the updater. It makes the shipped adapter the
architectural truth and separates authorization from later delivery evidence.

## Alternatives rejected

**Drop the public repository, or mirror it automatically.** The public repository
is the external contribution surface, and the split keeps secrets, signing, and
private validation on the maintainer side. Automatic mirroring would either leak
that boundary or lose the review point.

**Shorten the checklist.** All 68 items are wanted. The problem is that a human
executes them, not that they exist.

**Make the public required checks advisory.** They are the last gate before
users. Weakening them to reduce friction trades a real property for a schedule.

**Keep chasing CI logs.** Rejected by measurement: a tool that stops at the first
failing target reports one item per run, so the loop length equals the number of
sites. Running the same invocation locally reported the whole set at once.

**Enable a merge queue.** Right idea, wrong decision record. Both repositories
run strict required checks with **no merge queue**, so every merge moves the base
and leaves the next branch stale; auto-merge does not re-sync it. Landing a
sequence of fixes therefore costs an update-and-re-run cycle each, which is a
large share of what made the day long — and it is precisely the cost D1's parity
lanes would multiply.

A queue is the standard mechanism for that shape, but enabling it changes merge
mechanics for **every workspace in the monorepo**, not the client release path
this ADR governs, and the decision has stakeholders this registry does not cover.
It is filed separately rather than smuggled in here.

## Migration

Ordered so that each step is worth landing on its own.

1. **Verification parity for the required set** (D1). Removes the round trips.
   Highest value, and it makes every later step cheaper to validate.
2. **Checklist disposition registry and its guard** (D3). Turns 68 prose items
   into a machine-readable contract; nothing is deleted.
3. **Evidence identity binding** (D2). Removes the clock race.
4. **Toolchain pin across all target-install sites, plus the canary** (D4).
5. **Export promotion and the divergence refusal** (D5).
6. **Rehearsed reversal and a stated rollout stage** (D7). Deliberately *before*
   the single entry point: a command that ships faster without a tested way back
   makes the worst outcome cheaper to reach.
7. **Single release entry point** (D6), last: it composes the six above.

## Consequences

- Internal CI cost rises. Parity lanes duplicate work that the public repository
  already does; the trade is bounded and deliberate, and the round trips they
  remove were more expensive.
- The disposition registry becomes a maintained artifact. If it is allowed to
  fossilize it will be the next thing that reads as evidence while measuring
  nothing.
- The divergence refusal introduces friction on purpose: an unimported public
  commit blocks regeneration until a maintainer imports it. That is the intended
  behaviour, and it will be felt the first time an external contribution lands.
- Pinning the toolchain means new lints arrive on upgrade rather than
  continuously. The canary is what keeps that from becoming a surprise, and it
  only works while it runs the same invocations as the required check.
- **Releasing becomes a claimed operation.** Several agents merge to the default
  branch concurrently, and the ruleset is strict with no merge queue: a branch
  goes stale while its own checks run. D6's single command therefore needs a
  claim, or two releases will interleave and each will believe it holds the head
  it validated.
- **The parity duplication costs about 17 minutes per pull request.** Measured on
  2026-08-21 from the public `Check (fmt + clippy)` job, which runs the three
  invocations D1 would mirror: **17m05s green**, **5m16s red** — red is cheaper
  because clippy stops at the first failing target.

  The trade is therefore *17 minutes on every client pull request* against *an
  export round trip when something is wrong*. A round trip is parent merge →
  regenerate → public pull request → public CI, where the clippy job alone is
  those same 17 minutes and the `Test` job measured 55. That is a good trade only
  if the lane fires often enough. Today's cascade suggests it would: two of the
  three lints cleared (`useless_format`, `ok_expect`) had been in the tree
  already and surfaced only once compilation got past the first crate — a parity
  lane would have reported them without any export.

  This estimate is a public-CI hosted number. The internal lane runs on different
  hardware, so the first step must re-measure there rather than inherit it.
- **Credential expiry becomes a scheduled concern.** Weekly releases meet
  certificate and notarization lifetimes far more often than quarterly ones, and
  nothing today reports an approaching expiry before it blocks a tag.

## Non-goals

- Automating the hybrid import. `docs/guides/hybrid-import-workflow.md` requires
  at least five processed low-risk public pull requests first; this ADR does not
  shorten that.
- Changing who approves a public export. Maintainer approval stays.
- Any change to server-side architecture or its release path.
- **Examining the composition of the required status-check set.** This ADR takes
  the 15 as given and only asks that they be decided internally first (D1).

  That exclusion is worth naming rather than leaving implicit, because it is the
  same shape as the problem D3 attacks. A dispatch-only lane has to justify its
  existence — `scripts/ci/release_gate_registry.json` records a `why` for each
  one and a guard fails when a new lane appears in neither map. Nothing asks the
  same of a check that **blocks every merge**: no document states the criterion
  for making a check required, so that set grows by accretion exactly as the
  68-item checklist did.

  It is excluded here because the set gates the whole repository rather than the
  client release path, so changing it has stakeholders this registry does not
  cover — the same boundary that keeps the merge queue out. It should get the
  D3 treatment in its own decision.

## Amendments

None yet.
