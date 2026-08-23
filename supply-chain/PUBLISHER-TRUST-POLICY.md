# Publisher-trust policy

*ONESHIM / Maekon client — `supply-chain/audits.toml`. Issue #11278 (VD-05.4).*

This file states what has to be true, and what has to be written down, before a
`[[trusted.<crate>]]` entry is added to `audits.toml`. It exists because ~200 remaining
exemptions cannot be closed by reading code — several are over a million lines — so the
decision is about publishers, and a decision made 200 times without a stated rule is made
200 different ways.

Audits are preferred over trust wherever an audit is possible. Trust is what we use when it
is not.

---

## 1. What a trust entry actually claims, and what it does not

cargo-vet's own documentation is blunt about this, and the wording matters:

> A `[[trusted]]` entry declares that you trust the developer of a given crate to always
> release code which meets the desired criteria. **The trusted publisher is not consulted
> and may or may not have personally authored or reviewed all the code.** … Trusting a
> publisher is fundamentally a heuristic; assess the risk and potentially do some
> investigation on the development and release process before trusting a crate.

Two things follow.

**Trust is not an audit.** An `[[audits]]` entry says *we read this code*. `safe-to-deploy`
requires the auditor to "review sufficient code to reason completely about all unsafe blocks
and powerful imports". A trust entry says *we expect this person's future releases to be
fine*. Those are different claims and this file never lets one stand in for the other.

**The tool itself asks for investigation.** "Do some investigation on the development and
release process" is not optional colour — it is the difference between a recorded judgment
and a guess. Section 3 lists what that investigation must record.

## 2. Another organisation's trust is corroboration, never the basis

We import audits from ten registries (`config.toml` `[imports.*]`). `cargo vet suggest`
frequently reports that one of them already trusts a publisher:

    NOTE: isrg, zcash, arielos, mozilla, and 1 other trust Kenny Kerr (kennykerr)

It is tempting to treat that as sufficient. It is not, for two measured reasons.

**We do not inherit those decisions.** Measured on `imports.lock` (2026-08-22): 185
`[[audits.*]]` and 183 `[[publisher.*]]` records are imported, and **zero** `[[trusted.*]]`.
Other organisations' publisher-trust judgments are not part of our verdict. `suggest` is
telling us that *someone else made this call*, and inviting us to make our own.

**Their calls carry no reasoning we can read.** Measured on the same date, from the four
registries that back nearly every such NOTE in our backlog:

| registry | `[[trusted]]` entries | entries carrying `notes` |
|---|---:|---:|
| mozilla  | 228 | **2** |
| zcash    | 206 | **0** |
| arielos  |  95 | **0** |
| isrg     |  50 | **0** |
| *(ONESHIM, for contrast)* | *231* | ***231*** |

579 trust decisions across those four, 2 of them with a stated reason. Adopting them as our
basis would import conclusions with no argument attached, and would lower a standard this
repository already meets on every one of its own entries.

They are also not peers of each other: `mozilla` is an organisation-wide registry, while
`isrg` here is the supply-chain file of a single project (`divviup/libprio-rs`) and `arielos`
is an embedded-OS project carrying 95 trust entries against 40 audits. "Four organisations
agree" is a weaker statement than it sounds.

**So: cite them, count them, and do not lean on them.** A corroborating registry raises
confidence. It does not replace §3.

## 3. What must be recorded

A trust entry's `notes` must let a reader who was not in the room reconstruct the decision.
Five findings, each stated as a fact rather than a conclusion:

**(a) Publish share.** How many of the crate's releases this publisher shipped, and who
shipped the rest — `"26 of 85 releases; the remainder predate publisher records, and thomcc
is a co-maintainer"`. A publisher holding a small share is not disqualifying, but it changes
what the trust means and must be visible.

**Count the denominator before you read the share.** A share is a ratio, and three
separate mechanisms shrink the numerator or inflate the denominator without anything
being wrong with the publisher. Each of them makes a legitimate maintainer look
peripheral, so check all three before treating a low share as a finding.

*The denominator can predate the records.* crates.io only began storing `published_by`
around 2019. Every release before that returns null for both `published_by` and
`trustpub_data`, and counting those into the denominator understates every publisher on
the crate. In this file `notify` is the clear case: 42 of its 83 versions carry no
publisher record at all, and every one of them is dated 2019-02-09 or earlier. Over all
83 the maintainers read 27% / 10% / 7% / 3%; over the 41 versions that actually have
records they read 56% / 21% / 14% / 7%. Nothing changed but the denominator.

*Trusted Publishing releases are not counted as anyone's.* See (d) — a TP release has no
`published_by` login, so counting logins alone drops it into the same "no publisher"
bucket as the pre-2019 releases.

*Rotating release managers divide the share by construction.* A crate whose maintainers
take turns cutting releases can never give any single identity a high share, and the
healthier the maintainer bench the lower each share goes. `notify` shipped 8.0.0 under
one maintainer, 8.1.0 under a second, and 8.2.0 under the first again. Read that as a
release-management pattern, not as a thin claim on the crate.

State the corrected share and say which of these applied. A share reported without its
denominator is not a finding.

**(b) Window basis.** Why `start` is where it is. The window exists so that releases from a
previous owner stay outside the claim; say whose releases those were. `end` is a revisit
date, not an expiry of concern — see §5.

**(c) Corroboration.** Which registries trust the same publisher, if any, taken from
`cargo vet suggest`. Record the count and the names. Record also that those registries
publish no reasoning (§2) — so the reader does not over-read the citation.

**The crates.io owner list can omit the publisher entirely.** Ownership can be held by a
GitHub *team* as well as by individuals, and a maintainer who publishes through that team
does not appear in the user list. Checking "is this publisher an owner?" then returns *no*
for someone with full, legitimate authority over the crate — the corroboration path fails
in the direction that looks like a finding.

`notify` in this file is that shape. Its owners are `passcod`, `0xpr03`, `JohnTitor`, and
the team `github:notify-rs:watchmakers`. The 8.2.0 release in our tree was published by
`dfaust`, who is in none of the three user entries.

When the owner list does not account for the publisher, do not stop there and do not read
the absence as adverse. Go to the source repository, which can settle it directly:

    gh api /repos/<owner>/<repo>/contributors --jq '.[] | "\(.contributions) \(.login)"'
    gh api /repos/<owner>/<repo>/git/ref/tags/<tag> --jq '.object.sha'

For `dfaust` this returned 165 commits — second in the repository, ahead of two people who
*are* listed owners — and the commit that tag `notify-8.2.0` points at, `a1d7c2d8f`
"Prepare release (#706)", is authored by them on the same day crates.io recorded the
publish. That is stronger corroboration than an owner-list entry, because it ties the
identity to the specific release rather than to the crate in general.

Record which path settled it. "Not in the owner list" on its own is a statement about the
owner list, not about the publisher.

**(d) Release-process signal.** Whether the crate publishes through crates.io Trusted
Publishing. This is mechanically checkable:

    curl -s https://crates.io/api/v1/crates/<crate>/<version> | jq .version.trustpub_data

A non-null value pins that release to a specific repository and CI run
(`{"provider":"github","repository":"…","run_id":"…"}`); `null` means it was published with a
long-lived API token. Since 2026 a crate owner can *enforce* Trusted Publishing, which
disables token publishing entirely — the strongest available signal that a release could not
come from a leaked personal token.

Adoption is uneven across our own tree, so this has to be checked per crate rather than
assumed. Two samples measured on 2026-08-22:

- Fourteen large, long-established crates — `serde`, `wasi`, `windows_aarch64_gnullvm`,
  `rustls`, `rcgen`, `uuid`, `h2`, `clap`, `tokio`, `regex` and others — returned
  `trustpub_data` null on **every** version.
- The Tier 2 publisher-trust batch returned non-null on several: `cc` (39 of 220 versions),
  `find-msvc-tools` (11 of 12), `tar` (1 of 81), `mach2` (1 of 6).

A positive control (`zizmor` 39 of 83, `cargo-semver-checks` 7 of 79) confirms the field is
observable, so a null is a real null rather than a failed lookup. The pattern is that mature
high-traffic crates mostly predate the feature while newer and tooling-oriented crates have
adopted it — which is exactly why (d) is a per-crate lookup and not a standing assumption
about the tree.

**Presence changes which identity you must trust, not just how much.** A release published
through Trusted Publishing carries no `published_by` login — the identity on it is the
*signature*, written `github:owner/repo`. Trusting the human maintainer therefore does not
cover those releases at all, which is a silent failure: `cargo vet check` simply keeps the
exemption and nothing says why.

This was measured rather than reasoned about. In the Tier 2 batch, trusting eight human
publishers retired only five exemptions; the three that survived — `cc`, `find-msvc-tools`
and `mach2` — were the ones with the *highest* Trusted Publishing coverage. Trusting
`github:rust-lang/cc-rs` and `github:JohnTitor/mach2` retired all three immediately.

So when (d) comes back non-null, read the `repository` field and trust that signature.
A crate whose releases are split between a person and a signature needs **both** entries;
`cc` has exactly that shape in this file.

The same fact invalidates a naive publish-share count. Computing (a) from `published_by`
logins silently scores every Trusted Publishing release as "no publisher", which reads as a
low share for whoever you are considering. An earlier note in this initiative recorded that
`github:JohnTitor/mach2` had "published 0 of 6 versions" and skipped it on that basis; the
signature had in fact published the one version the tree needed. Count signatures alongside
logins, or the number means the opposite of what it appears to.

Absence is **not** disqualifying. It is recorded because its *presence* is strong evidence,
and because the day a given crate adopts it, that is worth knowing.

> An earlier revision of this section generalised the first sample to "our set simply does
> not use it yet". The second sample refuted that. The correction is kept visible because the
> failure mode is the one this repository's measurement rules warn about: a collection range
> narrower than the question inverts the answer.

**(e) Exposure class.** Whether the crate sits on a security boundary, decided by the test in
§4. This is the one finding that changes the outcome rather than the record.

## 4. The decision rule

A crate is **boundary** when this is true of it:

> A single malicious release would, on its own and without needing a second bug, give the
> attacker code execution or access to key material.

| Exposure | Publisher trust alone |
|---|---|
| **Boundary** | **Not sufficient.** Requires an audit, or an owner exception whose notes say why an audit was not performed and what was checked instead. |
| **General** | Sufficient, when (a)–(d) are recorded and the publisher's share is dominant or the remainder is accounted for. |

Three families have met that test so far. They are examples, not the definition:

| family | examples | why |
|---|---|---|
| Build-time code execution | `cc`, `cmake`, `jobserver`, `find-msvc-tools`, `openssl-src` | a malicious release runs arbitrary code on every developer and CI machine at build time |
| FFI signature surface | `libc`, `wasi`, `mach2` | these declare the signatures everything else calls; a wrong one is memory corruption by itself |
| Arbitrary-path write | `tar` | extraction writes to attacker-controlled paths |
| Cryptography and certificates | `rcgen`, `rustls`, `landlock` | key material and trust decisions |

The rule is not new; it is already how this repository reasons. The `rcgen` entry says as
much in its own words — *"this is a certificate-generation crate, so widening the window
needs a fresh review"* — and the `h2` entry explains why a line-by-line audit of an HTTP/2
framing state machine would not have been meaningful, which is exactly the kind of statement
an owner exception has to make.

**Falsification.** A rule that returns the same answer for everything is not a rule.

Applied to five existing entries, the answers differ:

| crate | share recorded | exposure | verdict |
|---|---|---|---|
| `rcgen` | no | boundary | audit or recorded exception |
| `rustls` | yes | boundary | audit or recorded exception |
| `landlock` | yes | boundary | audit or recorded exception |
| `schemars` | yes | general | publisher trust sufficient |
| `yoke-derive` | yes | general | publisher trust sufficient |

Applied to the eighteen crates of the Tier 2 batch (#11277), it split them **8 general / 10
boundary** rather than waving all eighteen through.

> An earlier revision stated the boundary as a list of topics ending in "process or
> filesystem capability". Read literally against low-level crates that swallowed almost
> everything — `winapi-util` wraps Windows file handles, `filetime` sets mtimes, `notify`
> watches directories, `tokio` does network and file I/O — and a rule that classifies
> everything the same way decides nothing. The five-entry falsification above had not caught
> it because those five were at the two extremes with no middle ground. The single-sentence
> test replaces the list, and the list becomes examples.

## 5. Approval, window, and revocation

**Approval.** A trust entry lands the same way every other change to this file lands: in a
pull request, reviewed, under `who = "ONESHIM Contributors <security@oneshim.thengd.com>"`.
Existing notes use the phrase *owner-approved identity* to mark that the publisher's identity
was confirmed by the repository owner; keep using it, and cite the issue where that happened.
There is no separate approval ledger — the PR is the record.

**Window.** `start` follows §3(b). `end` is a revisit date; entries in this file currently sit
around 2027. When it passes, `cargo vet check` fails and the entry is re-decided under this
policy — not extended by default. A revisit is a new decision and gets a new note.

**Revocation.** If a publisher account is compromised, or a release is found to be
malicious:

1. Delete the `[[trusted.<crate>]]` entries for that publisher. `cargo vet check` then fails
   for every version that depended on them — that failure is the point, and it is the signal
   that tells you the blast radius.
2. Do **not** restore an exemption to make the failure go away. An exemption says "we accept
   this unreviewed"; after a compromise that is the wrong claim. Pin to a known-good version,
   or audit the specific versions in the tree.
3. Record the revocation and its date in the PR that removes the entries, so that the next
   person to consider that publisher finds it.

## 6. Retroactive position of the existing entries

The 231 trust entries in this file predate this policy. Measured on 2026-08-22 against §3:

| finding | entries recording it |
|---|---:|
| (b) window basis — every entry carries `start` | **231 / 231** |
| (a) publish share | **10 / 231** |
| (c) corroboration | **0 / 231** |
| (e) boundary-class crates among them | 19 / 231 |

Most entries instead defer to an earlier bulk decision, in the form *"this publisher is
already trusted in this file for other crates under the #3908 owner-approved identity
review"*. That is a real decision with a traceable issue behind it, but it is not the
per-crate finding §3 asks for.

**These are not being rewritten.** The gap is recorded here so that it is known rather than
assumed, and so nobody reads an old entry as evidence that §3 was satisfied. The policy binds
new entries and re-decided entries. When an existing entry's window expires (§5) and it comes
up for revisit, it is brought up to §3 then.

The 19 boundary-class crates among the existing entries are the subset where the gap matters
most; they are the natural first candidates for an audit under VD-05 Tier 1.

---

## Sources

- cargo-vet, *Built-In Criteria Definitions* — <https://mozilla.github.io/cargo-vet/built-in-criteria.html>
- cargo-vet, *Trusting Publishers* — <https://mozilla.github.io/cargo-vet/trusting-publishers.html>
- RFC 3691, *Trusted Publishing for crates.io* — <https://rust-lang.github.io/rfcs/3691-trusted-publishing-cratesio.html>
- crates.io development update (2026-01) — <https://blog.rust-lang.org/2026/01/21/crates-io-development-update>
- Measurements in §2, §3(d), §4 and §6 were taken on 2026-08-22 against this repository's
  `imports.lock` and `audits.toml`, the four upstream registries named in `config.toml`, and
  the crates.io API.
