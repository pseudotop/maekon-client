# Database Encryption Guide

MAEKON stores all activity data in a local SQLite database. This guide documents
the encryption key management strategy.

## Key Storage

The key sits next to the database, in the application **data** directory
resolved by `ConfigManager::data_dir()`
(`crates/maekon-core/src/config_manager/path_resolution.rs`). Note that this is
not the same directory as the config file on any platform.

| Platform | Location |
|----------|----------|
| macOS | `~/Library/Application Support/maekon/data/.db_key` |
| Linux | `~/.local/share/maekon/.db_key` |
| Windows | `%LOCALAPPDATA%\maekon\data\.db_key` |

### Profile flavors — read this before deleting anything

The `maekon` path segment is the *app directory name*, and it is suffixed when
`MAEKON_APP_FLAVOR` is set: the directory becomes `maekon-<flavor>`
(`app_dir_name_for_flavor`). **Debug builds set `MAEKON_APP_FLAVOR=dev`
automatically** (`src-tauri/src/lib.rs`, `configure_runtime_flavor`) so a
locally built or `.app`-bundled debug client keeps its data out of the released
app's profile.

So on macOS a debug build actually uses:

```
~/Library/Application Support/maekon-dev/data/.db_key
```

Older revisions of this guide and of the README named
`~/Library/Application Support/com.maekon.app/`. That path has never been
correct: `com.maekon.app` is the macOS **bundle identifier**, not the data
directory — the code has always used `APP_DIR_NAME = "maekon"`. An operator
clearing or inspecting a profile at the old path was looking at nothing.

To see the directory a given build resolves, set the flavor explicitly rather
than guessing:

```bash
ls -la "$HOME/Library/Application Support/maekon-dev/data"   # debug builds
ls -la "$HOME/Library/Application Support/maekon/data"       # release builds
```

### A second, unrelated `.db_key`

`create_file_secret_store` (`src-tauri/src/provider_secret_backend.rs`) keeps
its own `.db_key` in the **config** directory
(`~/Library/Application Support/maekon/.db_key` on macOS). That one encrypts the
file-backed provider secret store, not the activity database, and is out of
scope for this guide. Do not confuse the two when backing up or clearing state.

## Key Properties

- **Algorithm**: AES-256 (32-byte key)
- **Source**: OS CSPRNG via `getrandom`
- **File permissions**: `0600` on Unix (owner read/write only)
- **Format**: Raw bytes (not hex-encoded on disk)

## Important

- Do NOT delete `.db_key` without backing up your data — the database cannot be recovered without the key.
- Back up the entire app data directory, including `.db_key`, to preserve data access.

## Implementation Status

Key generation and storage infrastructure is complete (`maekon_storage::encryption::EncryptionKey`).
Full at-rest encryption (SQLCipher integration) is planned as a follow-up.
