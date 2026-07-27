[English](./qc-upload-spool-recovery.md) | [한국어](./qc-upload-spool-recovery.ko.md)

# 격리 upload-spool 복구 fixture

이 문서는 `CRT-PRV-QC-CJ-05-05`용 debug-only fixture 절차를 설명합니다.
업로드 실패 후에도 row가 프로세스 경계를 넘어 pending으로 남고, 동일한
storage ID로 re-prime되며, synthetic adapter가 성공을 확인한 뒤에만 sent
marker가 기록되는지를 검증합니다.

이 fixture는 테스트 인프라입니다. production upload endpoint나 실제 서버
연동 성공 증거가 아닙니다.

## 안전 경계

- 새 전용 `qc-*` 또는 `tc-*` profile만 사용합니다. 재실행이 필요하면 해당
  profile만 삭제합니다.
- synthetic API client는 프로세스 내부에서만 동작하며 socket을 열지 않습니다.
- 준비 전에 capture, audio, sync, integration, telemetry, update 확인, 자동
  설치, external web/gRPC, automation을 모두 비활성화하고 UI 재시도 직전에
  격리 설정을 다시 검증합니다.
- 전용 profile의 암호화된 production SQLite adapter에 synthetic context event
  두 건만 기록합니다. 사용자 history는 읽지 않습니다.
- fixture 모듈, CLI dispatch, capability 범위 UI command는
  `debug_assertions`와 `analysis` feature에서만 컴파일됩니다. 일반 profile은
  fixture 상태를 반환하지 않고 release/no-analysis mutation은 fail-closed됩니다.
- 이 문서 실행만으로 Computer Use 결과를 PASS로 승격하지 않습니다. exact
  Windows build, bounded capture, Git LFS receipt, audit/egress delta, append-only
  JSONL row는 별도 증거 요건입니다.

## 필수 gate

두 단계 모두 아래 값을 정확히 설정합니다.

```text
MAEKON_DEBUG_QC_FIXTURE_CLI=1
MAEKON_TC_ISOLATED_PROFILE=1
MAEKON_DEBUG_QC_UPLOAD_SPOOL_FIXTURE=1
MAEKON_QC_UPLOAD_SPOOL_CONFIRM=interrupt-and-reprime
MAEKON_APP_FLAVOR=qc-8568-upload-spool
```

일반 flavor나 잘못된 flavor는 fail-closed됩니다. 준비 단계는 database 또는
fixture state file이 이미 있는 profile도 거부합니다.

## 1단계: 영속화, 실패, 중단

debug binary를 다음과 같이 실행합니다.

```text
maekon debug-prepare-qc-upload-spool
```

예상 결과:

- 암호화된 synthetic row 두 건이 pending 상태가 됩니다.
- synthetic upload 1회가 실패하고 volatile queue는 drop 없이 requeue됩니다.
- uploader를 제거해 프로세스 중단을 모사합니다.
- exact storage ID 두 개가 모두 pending으로 남고 confirmed ID는 0개입니다.
- `qc-upload-spool-state.json`에 `phase=interrupted`,
  `sent_markers_written_after_success=false`, egress ledger row 0,
  `host_mutation=false`가 기록됩니다.

중단 측 증거를 수집할 때는 여기서 프로세스를 종료합니다. 같은 프로세스
호출에서 2단계를 이어서 실행하지 않습니다.

## 2단계: 재시작, re-prime, 확인

동일한 exact gate와 profile로 새 프로세스를 시작합니다.

```text
maekon debug-verify-qc-upload-spool
```

UI에서도 확인하려면 동일한 gate와 profile로 debug 앱을 실행한 뒤 **설정 →
동기화**에서 **업로드 복구 점검** 패널의 **안전하게 다시 시도**를 누릅니다.
동일한 exact-ID 검증 경로를 사용하며, 패널은 대기 2/전송 완료 표시 0에서
대기 0/전송 완료 표시 2로 바뀌어야 합니다. 일반 profile과 release build에는
이 패널이 표시되지 않습니다.

예상 결과:

- SQLite pending row 두 건이 원래 storage ID와 함께 로드됩니다.
- synthetic success adapter가 그 exact ID를 반환합니다.
- `mark_as_sent` 직전까지 row는 여전히 pending입니다.
- 확인된 ID만 sent 처리되어 pending row가 0개가 됩니다.
- state가 `phase=verified`, `sent_markers_written_after_success=true`,
  egress ledger row 0으로 전환됩니다.

## 검증 명령

`clients/maekon-client`에서 실행합니다.

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

Tauri 컴파일에는
[`source-build-prerequisites.md`](../testing/source-build-prerequisites.md)에 설명된
ignored host-triple sidecar와 frontend dist 준비도 필요합니다.
