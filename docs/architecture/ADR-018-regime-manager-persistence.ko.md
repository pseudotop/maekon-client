[English](./ADR-018-regime-manager-persistence.md) | [한국어](./ADR-018-regime-manager-persistence.ko.md)

# ADR-018: RegimeManager 영속화

**상태 (Status)**: 채택됨 (Accepted)
**날짜 (Date)**: 2026-04-18
**범위 (Scope)**: `oneshim-core::ports::regime_storage`, `oneshim-storage::regime_manager_state_store`, `oneshim-analysis::RegimeManager::hydrate_from`, `src-tauri::main::RunEvent::Exit`

---

## 배경 (Context)

`RegimeManager` 는 완전히 인메모리(in-memory) 상태였다 — 재시작마다 사용자가 큐레이션한 regime 이름, 병합(merge), 삭제(delete) 내역이 모두 소실됐다. 기존 SQL `regimes` 테이블은 크로스 디바이스 동기화 경로(`sync_merger.rs`) 에서만 사용되며, RegimeManager 의 전체 상태(centroid, RegimeStatus enum, 사용자 지정 이름 오버라이드) 를 담고 있지 않다.

2026-04-16 Feature Gap Analysis (X6) 참조.

## 결정 (Decision)

`oneshim-core` 에 새 `RegimeStoragePort` 를, `oneshim-storage` 에 `SqliteRegimeManagerStateStore` 를 추가한다. 상태는 **전용(dedicated)** `regime_manager_state` 싱글톤(singleton) 테이블 (v31 마이그레이션) 의 JSON 블롭(blob) 으로 저장한다 — 기존 `regimes` 테이블은 사용하지 않는다.

시작 시 구성 루트(composition root) 가 `store.load_all()` → `RegimeManager::hydrate_from(regimes)` 를 호출한다. 정상 종료 시 `main.rs::RunEvent::Exit` 핸들러가 `store.save_all(&regime_manager.all_regimes())` 를 4 초 워치독(watchdog) 과 함께 호출한다.

파싱 실패 시 `load_all` 은 손상된 페이로드를 `payload_backup` 컬럼으로 격리(quarantine) 하며, `payload_backup_at` 타임스탬프를 기록하고 `error!` 로그를 남긴 뒤 `Ok(vec![])` 를 반환하여 앱은 새 상태로 시작한다. 사용자 큐레이션 상태는 이후 복구를 위해 보존된다.

## 결과 (Consequences)

### 긍정 (Positive)

- Regime 이 재시작에도 살아남는다 — 같은 클러스터에 대해 매 콜드 부트마다 "새 regime 발견" 알림이 재발생하지 않는다.
- Vector `regime_id` 필터 (C3a) 가 세션 경계를 넘어 의미를 갖는다 — regime ID 가 이제 안정적(stable) 이다.
- sync_merger 가 사용하는 기존 `regimes` 테이블은 건드리지 않는다.

### 부정 / 제약 (Negative / Constraints)

- JSON 블롭은 `Regime` 구조체의 진화에 따라 바뀐다. 현재 구조체는 `#[serde(default)]` 를 **전혀** 갖지 않으므로, 필드 추가/제거/이름 변경 어느 쪽이든 모두 격리 경로(quarantine path) 를 트리거한다. 필드 단위로 `#[serde(default)]` 를 추가하는 것은 의도적인 결정이어야 한다 — 일괄 적용은 버전 간 묵시적 기본값 치환(silent default-substitution) 을 숨겨서 실제 마이그레이션 의도를 가린다. 스키마 불일치는 절대로 조용히 wipe 되지 않는다 — 격리가 기존 payload 를 보존한다.
- `load_all` 은 격리 엣지 케이스에서 read-only 가 아니다. 문서가 호출자에게 경고하며, 모든 호출 지점은 시작 시 단발(single-shot) 이다.
- 종료 시 저장은 best-effort 이다. 워치독은 2 계층이며 각 계층에 한계가 있다:
  1. `tokio::time::timeout(4s)` 가 save future 를 감싼다. 그러나 `SqliteRegimeManagerStateStore::save_all` 은 `std::sync::Mutex<Connection>` 락을 잡고 `rusqlite::Connection::execute` 를 호출한다 — 둘 다 블로킹 sync 이며, 내부에 `.await` 지점이 없다. tokio 의 timeout 은 `.await` 경계에서만 poll 되므로 in-flight SQL 을 선점(preempt) 할 수 없다. timeout 은 save 가 mutex 획득 전에 yield 하거나(예: 런타임 스레드 대기) 내부 채널 로직이 await 할 때만 발동한다.
  2. 메인 스레드의 `std::sync::mpsc::recv_timeout(4.5s)`. 이 쪽은 실제로 4.5 초에 발동해 종료를 진행시킨다. 진짜로 멈춘 save 스레드는 이 대기를 넘겨 살아남으며, 프로세스 종료 시 OS 가 회수한다.
  실제로 SQL 은 작은 JSON blob + `INSERT OR REPLACE` 로 정상 디스크에서는 <50 ms 에 완료된다. SQLite 의 journal 이 torn-write 리스크를 방지한다 — `execute` 는 커밋됐거나(WAL 에 내구성 있게 기록) 아예 커밋되지 않거나(다음 open 시 journal 이 롤백) 둘 중 하나다.
- **시그널 기반 종료(SIGINT/SIGTERM) 는 저장 자체를 건너뛴다.** `lifecycle.rs::wait_second_signal` 은 `FORCE_EXIT_GRACE_SECS` 이후 `std::process::exit(0)` 를 직접 호출하며, Tauri 의 `RunEvent::Exit` 클로저보다 먼저 실행된다. `kill -TERM <pid>`, `launchctl unload`, 또는 tray-quit 이 아닌 모든 종료 경로는 regime save 와 suggestion-queue save 를 모두 건너뛴다. 이는 본 ADR 이 도입한 것이 아니라 기존 동작(suggestion-queue save 도 동일 제약) 이며, "graceful shutdown" 이라는 표현의 엄밀한 해석이 이를 가릴 수 있어 명시한다. 런타임 주기적 저장(아래 Neutral) 이 후속 해결 방안이다.
- **종료 순서 주의.** `RunEvent::Exit` 는 WAL 체크포인트를 regime save **앞에** 실행한다. 순서를 뒤집으면, 멈춘 save 가 connection mutex 를 보유한 상태에서 같은 `Arc<Mutex<Connection>>` 에 접근하려는 체크포인트가 블록돼 WAL 이 truncate 되지 않는다. 체크포인트를 먼저 돌려 unblocked window 를 확보하고, 이어지는 save 는 새 WAL 에 쓰기만 한다 — 프로세스가 쓰기 도중 사망해도 다음 시작 시 WAL 이 idempotent 하게 replay 된다.

### 중립 (Neutral)

- 런타임 주기적 저장(mid-life periodic save) 은 본 페이즈 범위 밖이다. 루틴 재시작 생존을 위해서는 종료 시점 저장만으로 충분하며, 콜드 킬(cold-kill) 데이터 손실이 문제가 되면 `run_maintenance` 틱 이후 저장을 추가하는 후속 페이즈가 가능하다.

## 대안 검토 (Alternatives considered)

- **기존 `regimes` 테이블 재사용** — 기각. 스키마가 부분적(centroid 없음, RegimeStatus enum 없음, 사용자 이름 오버라이드 없음) 이며 sync_merger 소유다. 확장하려면 마이그레이션과 쓰기 경로 업데이트가 필요해 동기화 일관성을 유지해야 한다. 신규 전용 테이블이 블라스트 반경(blast radius) 을 피한다.
- **JSON 블롭 대신 regime 별 행(row-per-regime)** — 기각. RegimeManager 의 regime 수는 `max_active + archive_days` 로 제한되므로 단일 블롭이 단순하며 비용은 무시 가능하다. 필요해지면 diff API 를 백워드 호환으로 후속 추가할 수 있다.
- **"파싱 실패 시 fresh start"** — 스펙 리뷰 중 명시적으로 기각. 수개월의 사용자 큐레이션 이름을 조용히 wipe 하는 것은 회귀(regression) 다. 격리(quarantine) 가 복구 경로를 보존한다.

## 참조 (References)

- 구현 기록: 내부 regime feedback learning 명세와 feature-gap 분석 노트
- ADR-016 ConfigChangeBus (종료 워치독 패턴)
