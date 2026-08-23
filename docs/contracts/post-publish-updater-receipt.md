# Post-Publish Updater Receipt Contract

`RC-MANUAL-006` is a post-publication observation. It cannot honestly be a
pre-tag release-decision pass because the new GitHub Release does not exist
until after the signed tag is pushed and the release workflow publishes it.

The pre-tag manifest therefore records this item as `phase=post_publish` and
`state=pending`. That pending state is visible but does not block creation of
the release candidate. The release candidate is not operationally complete
until a previous RC, configured for the prerelease channel, observes the new
tag and this receipt validates.

## Schema

The receipt uses `maekon.post_publish_updater_receipt.v1` and contains:

- exact `release_tag`, `release_commit_sha`, and `detected_tag`;
- a lower `previous_release_tag` with the same base version and its exact
  `previous_release_commit_sha`;
- `channel=prerelease`, `result=available`, and
  `detection_source=previous-rc-runtime`;
- UTC `observed_at` and a non-empty `observer`;
- one sanitized evidence artifact with a SHA-256 digest and shareable privacy
  and redaction states.

Mock API tests prove implementation behavior but are not accepted as this
runtime observation. A current-RC binary observing itself is also not accepted.

## Validation

Fetch the exact published tag, then run:

```bash
scripts/verify-post-publish-updater-receipt.sh v0.0.1-rc.10 <receipt.json>
```

The wrapper resolves both local tags to their commits and passes those values
to the validator. Missing tags, mismatched commits, wrong channels,
non-preceding source RCs, stale observations, and unsafe evidence fail closed.

## Receipt shape

```json
{
  "schema_version": "maekon.post_publish_updater_receipt.v1",
  "release_tag": "v0.0.1-rc.10",
  "release_commit_sha": "<40 lowercase hex>",
  "previous_release_tag": "v0.0.1-rc.9",
  "previous_release_commit_sha": "<40 lowercase hex>",
  "channel": "prerelease",
  "result": "available",
  "detected_tag": "v0.0.1-rc.10",
  "detection_source": "previous-rc-runtime",
  "observed_at": "<UTC timestamp ending in Z>",
  "observer": "<release maintainer>",
  "evidence": {
    "uri": "artifact://<privacy-safe evidence id>",
    "sha256": "<64 lowercase hex>",
    "privacy_status": "redacted",
    "redaction_status": "redacted",
    "sanitized": true
  }
}
```
