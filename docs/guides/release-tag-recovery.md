# Release Tag Recovery

**Audience**: the maintainer who has already pushed a `v*` tag and then watched
the release job fail.

A pushed tag is the point where a release stops being reversible cheaply. This
guide exists because the recovery that was actually performed on
`v0.0.1-rc.9` was manual and written down nowhere — and it only worked because
of a condition nobody checked deliberately at the time.

## The boundary that decides everything

**Have any release assets been published?**

Everything below hangs on that single question. Deleting a tag whose release
published nothing is routine. Deleting a tag after assets exist is a different
act entirely: anyone who downloaded an asset holds bytes that claim to come from
that tag, and re-creating the tag at a different commit makes those bytes
unverifiable rather than merely stale.

Answer it before touching anything:

```bash
gh api repos/pseudotop/maekon-client/releases/tags/vX.Y.Z \
  --jq '{id, draft, prerelease, assets: (.assets | length)}'
```

- **HTTP 404** — no release object exists. Nothing was published.
- **`assets: 0`** — the release object exists but is empty.
- **`assets: N` where N > 0** — assets are published. Stop and read
  [After assets are published](#after-assets-are-published).

A 404 and `assets: 0` are not the same state, but both are safe to recover from
by replacing the tag.

## Case 1 — the release published nothing

This is what happened with `v0.0.1-rc.9`: the tag was created without a
signature, `release.yml` refused it at `Verify release source trust`, and the job
stopped before producing anything.

```
##[error]v0.0.1-rc.9 tag signature is not GitHub-verified (reason: unsigned).
```

### Fix the cause before re-tagging

Re-creating the tag the same way reproduces the same failure. Establish that this
checkout can actually sign, rather than that it is configured to:

```bash
./scripts/pre-release-check.sh X.Y.Z
```

The `[Tag Signing]` section signs throwaway bytes with the configured key. A
`user.signingkey` that is set but unreadable, unlocked, or absent fails there —
which is the whole point, because reading the config would have passed.

### Replace the tag

```bash
git tag -d vX.Y.Z
git push origin :refs/tags/vX.Y.Z
./scripts/publish-rc-tag.sh X.Y.Z          # or publish-stable-tag.sh
```

**Do not hand-roll the tag.** `publish-rc-tag.sh` creates it with `git tag -s`;
`git tag -a` produces an annotated tag that is *not* signed, and the release job
rejects it identically. Creating the tag through the REST API bypasses signing
altogether — that is how `v0.0.1-rc.9` came to be unsigned in the first place,
despite the checklist already saying not to tag directly.

### Confirm the replacement is actually signed

```bash
gh api repos/pseudotop/maekon-client/git/refs/tags/vX.Y.Z --jq '.object.sha'
gh api repos/pseudotop/maekon-client/git/tags/<sha> --jq '.verification'
```

`verification.verified` must be `true`. A `reason` of `unsigned` means the tag
was created outside the signing path again.

## After assets are published

**Do not delete the tag.** Published assets are the thing users may already
hold; the tag is what they would verify against.

Prefer, in order:

1. **Ship a new version.** A superseding `vX.Y.Z+1` costs a version number and
   leaves every existing download verifiable. This is almost always right.
2. **Mark the release as a draft or prerelease** so it stops being offered,
   while leaving the tag and its assets addressable.
3. **Delete individual assets** if a specific artifact is wrong but the tag and
   the rest of the release are sound.

Deleting the tag is a last resort and needs its own decision with the
maintainer — it is not covered here, because doing it safely depends on what was
downloaded and by whom, which this guide cannot know.

## Why the pre-flight cannot catch everything

`pre-release-check.sh` proves this environment *can* sign. It cannot prove the
tag *will be* signed, because the tag does not exist yet and the operator may
create it by some other route. The signature check in `release.yml` is the
backstop, and it necessarily runs after the tag is irreversible.

That ordering is not a defect to be engineered away; it is why this document
exists. The same shape produced #10698, where a Windows PE closure check first
ran after `v0.0.1-rc.7` was tagged and left an irreversible tag with no
artifacts.

## Related

- `docs/release-checklist.md` — Sign-off section names the only supported tag
  creation paths
- `docs/architecture/ADR-035-release-pipeline-determinism.md` — why verification
  parity and rehearsed rollback are treated as release-pipeline requirements
- `docs/guides/updater-rollback-windows.md` — rolling back an update that has
  already reached clients, which is a different problem from an unusable tag
