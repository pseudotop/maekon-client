# Version Migration Guide

This guide is for operators upgrading Maekon deployments. It covers versioning policy, config file locations, database schema migrations, rollback procedures, and the breaking change log.

---

## Semantic Versioning Policy

Maekon follows [Semantic Versioning 2.0.0](https://semver.org/):

- **Patch** (`x.y.Z`): Bug fixes, security patches. No config or schema changes. Safe to deploy without testing in staging first.
- **Minor** (`x.Y.z`): New features, new optional config fields. Backward-compatible. Existing config files continue to work; new fields default to safe values via `#[serde(default)]`.
- **Major** (`X.y.z`): Breaking changes. May include config field renames, schema restructuring, or removed features. Read the CHANGELOG and this guide before upgrading.

All releases are tagged `vX.Y.Z` on GitHub. The CHANGELOG entry for each version is required before release (enforced by CI).

---

## Config File Locations

The config file is JSON and is read at startup. If absent, a default file is written automatically.

| Platform | Config file path | Data directory |
|----------|-----------------|----------------|
| macOS | `~/Library/Application Support/oneshim/config.json` | `~/Library/Application Support/oneshim/data/` |
| Windows | `%APPDATA%\oneshim\config.json` | `%LOCALAPPDATA%\oneshim\data\` |
| Linux | `~/.config/oneshim/config.json` | `~/.local/share/oneshim/` |

Source: `crates/oneshim-core/src/config_manager.rs`, `config_dir()` and `data_dir()` functions.

### Config Backup Before Upgrade

Always back up `config.json` before a major version upgrade:

```bash
# macOS / Linux
cp ~/Library/Application\ Support/oneshim/config.json \
   ~/Library/Application\ Support/oneshim/config.json.bak

# Windows (PowerShell)
Copy-Item "$env:APPDATA\oneshim\config.json" `
          "$env:APPDATA\oneshim\config.json.bak"
```

---

## Database Schema Migrations

Maekon uses SQLite for local storage. The database file is located in the data directory above (filename: `oneshim.db` by default, overridable via `storage.db_path` in config).

Schema migrations run automatically at startup via `crates/oneshim-storage/src/migration.rs`. The migration runner is idempotent — running it twice on the same database is safe.

### Schema Version History

| Version | Tables / Changes |
|---------|-----------------|
| V1 | `events`, `frames` — core event and frame capture tables |
| V2 | `frames.file_path` column added |
| V3 | `system_metrics`, `system_metrics_hourly` — CPU/memory/disk/network metrics |
| V4 | `process_snapshots`, `idle_periods`, `session_stats`; window geometry columns added to `frames` |
| V5 | `tags`, `frame_tags` — tagging system |
| V6 | `work_sessions`, `interruptions`, `focus_metrics`, `local_suggestions` — edge intelligence |
| V7 | Composite indexes for query performance (`idx_events_sent_timestamp`, `idx_work_sessions_state_started`, `idx_interruptions_not_resumed`, `idx_focus_metrics_date_score`, `idx_suggestions_pending`) |

**Current version**: V7

All migrations use `CREATE TABLE IF NOT EXISTS` and `ALTER TABLE ADD COLUMN` patterns, so they are non-destructive. An existing database at any version below V7 will be migrated forward automatically. There is no data loss during migration.

### Verifying Migration Success

After startup, the schema version can be confirmed by inspecting the database:

```bash
sqlite3 ~/Library/Application\ Support/oneshim/data/oneshim.db \
  "SELECT MAX(version) FROM schema_version;"
# Expected output: 7
```

---

## Upgrade Procedure

### Standard Upgrade (patch or minor)

1. Stop Maekon (quit from system tray or kill the process).
2. Install the new version using your platform's installer.
3. Start Maekon. Schema migrations run automatically at startup.
4. Verify the application starts and the dashboard loads at `http://localhost:10090`.

### Major Version Upgrade

1. Read the CHANGELOG for the target version — look for `BREAKING CHANGE` entries.
2. Back up `config.json` and `oneshim.db`.
3. Review config fields against this guide's breaking change log below.
4. Stop Maekon.
5. Install the new version.
6. If config fields have changed, update your `config.json` accordingly.
7. Start Maekon and verify startup logs for migration errors.

---

## Rollback Procedure

If an upgrade fails or causes issues:

1. Stop Maekon.
2. Restore the previous version's binary (keep old installers in your deployment system).
3. Restore `config.json` from backup.
4. Restore `oneshim.db` from backup if the new version ran migrations that are incompatible with the old binary.

**Important**: Schema migrations are not reversible. If you roll back to a version that predates a migration, the old binary may fail to start or may ignore unknown columns. Always restore the database backup when rolling back across a schema version boundary.

---

## Breaking Change Log

### Consent Schema (backward-compatible)

The `ConsentRecord` schema in `crates/oneshim-core/src/consent.rs` was extended with two new fields:

- `revoked_at: Option<DateTime<Utc>>` — timestamp recorded when the user revokes consent (GDPR Article 17 audit trail)
- `data_deletion_requested: bool` — signals that queued data must be purged before the next upload cycle (GDPR Article 17, right to erasure)

Both fields use `#[serde(default)]`, so existing consent records written before these fields existed deserialize correctly with both fields set to their defaults (`None` and `false` respectively). No migration or manual action is required.

This change is fully backward-compatible. Old consent files remain valid; new fields activate only when consent is revoked.

---

## Minimum Allowed Version Floor

The config field `update.min_allowed_version` (optional string, semver) sets a floor below which the auto-updater will not install. This is used to enforce that a security-critical version is not bypassed during rollback.

In enterprise deployments, set this field in your managed `config.json` to the minimum version your security policy requires:

```json
{
  "update": {
    "enabled": true,
    "min_allowed_version": "1.2.0"
  }
}
```

---

## Getting Help

- Check [CHANGELOG.md](../../CHANGELOG.md) for version-specific notes.
- Open a GitHub issue for migration failures not covered by this guide.
- See [SECURITY.md](../../SECURITY.md) for security vulnerability reporting.
