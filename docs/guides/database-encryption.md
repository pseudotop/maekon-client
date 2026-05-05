# Database Encryption Guide

MAEKON stores all activity data in a local SQLite database. This guide documents
the encryption key management strategy.

## Key Storage

| Platform | Location |
|----------|----------|
| macOS | `~/Library/Application Support/com.maekon.app/.db_key` |
| Linux | `~/.local/share/maekon/.db_key` |
| Windows | `%APPDATA%\maekon\.db_key` |

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
