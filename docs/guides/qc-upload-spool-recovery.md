[English](./qc-upload-spool-recovery.md) | [한국어](./qc-upload-spool-recovery.ko.md)

# Isolated upload-spool recovery fixture

This guide covers the debug-only fixture for `CRT-PRV-QC-CJ-05-05`. It proves
that a failed upload remains pending across a process boundary, re-primes the
same storage IDs, and writes sent markers only after the synthetic adapter
confirms success.

The fixture is test infrastructure, not a production upload endpoint or proof
of a live server integration.

## Safety boundary

- Use a fresh, dedicated `qc-*` or `tc-*` profile. Delete only that profile if
  the fixture needs to be reset.
- The synthetic API client is in-process and never opens a socket.
- Capture, audio, sync, integrations, telemetry, update checks, automatic
  installation, external web/gRPC, and automation are disabled before
  preparation and revalidated before an interactive retry.
- The fixture writes two synthetic context events to the profile's encrypted
  production SQLite adapter. It does not read user history.
- The fixture module, CLI dispatch, and capability-scoped UI commands are
  compiled only with `debug_assertions` plus the `analysis` feature. Ordinary
  profiles return no fixture status; release/no-analysis mutation fails closed.
- Do not promote a Computer Use result from this guide alone. The exact Windows
  build, bounded capture, Git LFS receipt, audit/egress delta, and append-only
  JSONL row remain separate evidence requirements.

## Required gates

Set all values exactly for both phases:

```text
MAEKON_DEBUG_QC_FIXTURE_CLI=1
MAEKON_TC_ISOLATED_PROFILE=1
MAEKON_DEBUG_QC_UPLOAD_SPOOL_FIXTURE=1
MAEKON_QC_UPLOAD_SPOOL_CONFIRM=interrupt-and-reprime
MAEKON_APP_FLAVOR=qc-8568-upload-spool
```

Normal or malformed flavors fail closed. Preparation also rejects any profile
that already has a database or fixture state file.

## Phase 1: persist, fail, and interrupt

Run the debug binary with:

```text
maekon debug-prepare-qc-upload-spool
```

Expected result:

- two encrypted synthetic rows are pending;
- one synthetic upload attempt fails and the volatile queue is requeued without
  drops;
- destroying the uploader simulates process interruption;
- both exact storage IDs remain pending and zero sent IDs are recorded;
- `qc-upload-spool-state.json` has `phase=interrupted`,
  `sent_markers_written_after_success=false`, zero egress ledger rows, and
  `host_mutation=false`.

Stop after this phase when collecting the interruption-side evidence. Do not
reuse the same process invocation for Phase 2.

## Phase 2: restart, re-prime, and confirm

Start a new process with the same exact gates and profile:

```text
maekon debug-verify-qc-upload-spool
```

For an interactive UI check, launch the debug app with the same gates and
profile, open **Settings → Sync**, and select **Retry safely** in the **Upload
recovery check** panel. This invokes the same exact-ID verification path; the
panel must change from two pending/zero sent markers to zero pending/two sent
markers. Ordinary profiles and release builds do not render the panel.

Expected result:

- the two pending rows are loaded from SQLite and paired with their original
  storage IDs;
- the synthetic success adapter returns those exact IDs;
- the rows are still pending immediately before `mark_as_sent`;
- only the confirmed IDs are marked sent, leaving zero pending rows;
- state advances to `phase=verified` with
  `sent_markers_written_after_success=true` and zero egress ledger rows.

## Verification commands

From `clients/maekon-client`:

```bash
cargo test -p maekon-app --lib qc_upload_spool::tests -- --nocapture
cargo test -p maekon-app --test qc_upload_spool_contract -- --nocapture
cargo test -p maekon-app --test ipc_command_contract \
  crt_prv_qc_recovery_fixture_cli_is_debug_only -- --exact --nocapture
cargo test -p maekon-app --lib scheduler::loops::network::tests::reprime_ -- --nocapture
cargo test -p maekon-network --lib batch_uploader::tests::flush_returns_exact_storage_ids_of_uploaded_batch -- --exact --nocapture
cargo test -p maekon-storage --lib sqlite::tests::mark_as_sent_affects_pending -- --exact --nocapture
cargo fmt -p maekon-app -- --check
cd crates/maekon-web/frontend
pnpm vitest run src/pages/setting-tabs/SyncTab.test.tsx
pnpm exec tsc --noEmit
```

Tauri compilation also requires the ignored host-triple sidecar and frontend
dist prerequisites documented in
[`source-build-prerequisites.md`](../testing/source-build-prerequisites.md).
