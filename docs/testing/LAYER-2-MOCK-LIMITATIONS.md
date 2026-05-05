# Layer 2: Mock IPC Test Limitations

## What Layer 2 Tests

Layer 2 uses **Vitest + `@tauri-apps/api/mocks`** to test frontend IPC interactions.
These tests run in Node.js, NOT in a Tauri webview.

## The Fundamental Limitation

**Layer 2 tests the mock, not the real Rust contract.**

When you write:
```typescript
mockIPC((cmd) => {
  if (cmd === 'get_settings') return { monitoring: { interval: 10 } }
})
const settings = await invoke('get_settings')
expect(settings.monitoring.interval).toBe(10)
```

You are testing that:
1. The mock returns the expected shape ✅
2. The frontend code handles the shape correctly ✅

You are NOT testing that:
1. The real Rust `get_settings` command returns this shape ❌
2. The real redaction logic works ❌
3. Error cases match real Tauri behavior ❌

## Why This Matters

If someone changes the Rust command's return type (e.g., renames `interval` to
`interval_seconds`), Layer 2 tests will **still pass** because they test the mock.
Only Layer 1 (Rust `#[cfg(test)]`) and Layer 4 (WDIO against real binary) catch this.

## Drift Detection Strategy

| Layer | What It Catches | Authority |
|-------|----------------|-----------|
| **Layer 1** (Rust `#[cfg(test)]`) | Contract constants, pure logic | Authoritative |
| **Layer 2** (Vitest mocks) | Frontend handling of expected shapes | Informational |
| **Layer 3** (Playwright) | UI rendering + page interactions | Informational |
| **Layer 4** (WDIO + real binary) | End-to-end IPC contract | Authoritative |

For security-critical contracts (e.g., `ALLOWED_KEYS`):
- **Layer 1**: `allowed_keys_matches_expected_set` — fails if Rust constant changes
- **Layer 4**: `T140: get_allowed_setting_keys returns exact expected set` — fails if IPC response changes
- **Layer 2**: Uses denylist approach (rejects known-bad keys) instead of duplicating the allowlist

## When to Use Layer 2

✅ **Good use cases:**
- Verifying frontend handles success/error cases
- Testing UI state transitions after IPC calls
- Regression testing frontend-specific bugs

❌ **Bad use cases:**
- Validating the exact IPC contract shape (use Layer 1 or 4)
- Testing security boundaries (use Layer 1 or 4)
- Duplicating Rust logic in TypeScript (maintenance burden)

## Adding New IPC Mock Tests

When adding a new Tauri command:
1. Add Rust unit tests in `#[cfg(test)]` (Layer 1) — authoritative
2. Add mock IPC test in `src/__tests__/ipc/` (Layer 2) — for frontend validation
3. Add WDIO contract test (Layer 4) — if security-critical
4. **Never** duplicate constants from Rust into TypeScript mocks
