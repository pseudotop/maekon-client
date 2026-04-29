# ADR-006: Tauri IPC Command Contract

**Date**: 2026-03-08
**Status**: Accepted
**Deciders**: Maekon maintainers
**Related**: [ADR-005: Tauri v2 Governance](ADR-005-tauri-governance.md)

---

## Context

Tauri v2 exposes Rust functions to the JavaScript frontend via the `tauri::generate_handler!` macro (registered through the `.invoke_handler()` builder method). This creates a typed IPC surface between the Rust backend (`src-tauri/src/commands.rs`) and the React frontend. Without a documented contract, IPC calls become an implicit API that is difficult to audit, version, or test.

This ADR documents the current IPC surface, defines the error handling pattern, and establishes the versioning policy for breaking changes.

---

## Decision

All Tauri IPC commands are defined in `src-tauri/src/commands.rs` and registered in `src-tauri/src/main.rs` via `tauri::generate_handler!`. The full set of registered commands is the authoritative IPC surface. No other entry points exist.

---

## Current Command Surface

As of 2026-03-08, the following commands are registered:

### `get_metrics`

Returns current system and agent resource usage.

**Input**: none

**Output**:
```typescript
{
  agent_cpu: number;       // Agent process CPU usage (%)
  agent_memory_mb: number; // Agent process memory (MB)
  system_cpu: number;      // Total system CPU usage (%)
  system_memory_used_mb: number;
  system_memory_total_mb: number;
}
```

**Errors**: String — `sysinfo` failure (rare; returns zero values on partial failure)

---

### `get_settings`

Returns the current `AppConfig` as a JSON object.

**Input**: none

**Output**: Full `AppConfig` JSON object. Shape matches `crates/oneshim-core/src/config/mod.rs` (`AppConfig` struct).

**Errors**: String — serialization failure (should never occur in practice)

---

### `update_setting`

Applies a partial config patch. Only allowlisted top-level keys are accepted; all others return an error.

**Input**:
```typescript
config_json: string  // JSON string containing the partial config object
```

**Allowed top-level keys** (enforced server-side; any other key returns an error):
- `monitoring`
- `capture`
- `notification`
- `web`
- `schedule`
- `telemetry`
- `privacy`
- `update`
- `language`
- `theme`

The patch is deep-merged into the current config. Keys not present in the patch are preserved.

**Output**: `void`

**Errors**: String — invalid JSON, disallowed key, or config serialization failure

**Security note**: Keys such as `server`, `sandbox`, `ai_provider`, `file_access`, and `grpc` cannot be modified from the WebView. They can only be changed by editing `config.json` directly (requires OS-level access to the config directory).

---

### `get_update_status`

Returns the current state of the auto-updater.

**Input**: none

**Output**: JSON object with `phase` field. When updates are disabled: `{"phase": "Disabled", "message": "Updates disabled"}`. When enabled, the phase reflects the updater state machine (e.g., `Idle`, `Checking`, `Available`, `Downloading`, `Ready`).

**Errors**: String — serialization failure

---

### `approve_update`

Triggers installation of a pending update. The user must confirm before calling this command.

**Input**: none

**Output**: `void`

**Errors**: String — no pending update, or update action channel closed

---

### `defer_update`

Defers a pending update to the next check interval.

**Input**: none

**Output**: `void`

**Errors**: String — update action channel closed

---

### `get_automation_status`

Returns whether the automation controller is configured and active.

**Input**: none

**Output**: `boolean` — `true` if the `AutomationController` is initialized

**Errors**: String (none expected in practice)

---

## Error Handling Pattern

All commands return `Result<T, String>`. Errors are string-serialized for JavaScript consumption. The frontend should treat any non-null error string as a failure and display it to the user or log it.

```typescript
// Idiomatic frontend usage
const result = await invoke<MetricsResponse>('get_metrics');
// Tauri throws on Err(_) — wrap in try/catch
```

Errors from infrastructure (sysinfo failures, config I/O errors) are wrapped into strings at the command boundary. No structured error codes are used in the current version.

---

## Security Model

IPC commands are only callable from the embedded WebView frontend. The Tauri security model enforces this at the process level — no external process, no network request, and no browser extension can invoke these commands.

The `update_setting` command enforces an allowlist at the Rust layer (not in JavaScript). The frontend cannot bypass this check by constructing a raw IPC message.

Sensitive config sections (`server`, `grpc`, `ai_provider`, `sandbox`, `file_access`) are intentionally excluded from the allowlist. These are administrator-managed fields configured via the file system, not user-controlled via the UI.

---

## Versioning Policy

### Non-Breaking Changes (allowed without version bump)

- Adding a new field to an existing command's output (with `#[serde(default)]` on the Rust side)
- Adding a new command to the handler (new commands are additive)
- Changing internal implementation without changing input/output types

### Breaking Changes (require major version bump)

- Removing a command
- Renaming a command
- Changing an existing command's input type in a non-backward-compatible way
- Changing an existing command's output type in a non-backward-compatible way
- Removing a field from a command's output without a deprecation period

When a breaking change is necessary:
1. Increment the major version in `Cargo.toml` workspace `version`.
2. Update this ADR with the new command surface.
3. Update `CHANGELOG.md` with a `BREAKING CHANGE` entry.
4. Notify downstream teams before releasing.

---

## Adding New Commands

To add a new IPC command:

1. Define the function in `src-tauri/src/commands.rs` with `#[command]`.
2. Add the function to the `tauri::generate_handler![]` call in `src-tauri/src/main.rs`.
3. Update this ADR with the new command's input, output, and error contract.
4. Add a TypeScript type declaration for the response shape in the frontend.

Do not add commands that expose file system paths, process lists, or network configuration to the frontend. Route those through the existing `get_settings` / `update_setting` pattern.
