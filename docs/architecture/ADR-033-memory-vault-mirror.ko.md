[English](./ADR-033-memory-vault-mirror.md) | [한국어](./ADR-033-memory-vault-mirror.ko.md)

# ADR-033: Memory Vault Mirror — 사용자 소유 로컬 마크다운 표면

**상태**: Accepted — 3-loop 리뷰(#9465) 후 2026-07-29 개정·승인; 원 Proposed 2026-07-29
**Date**: 2026-07-29
**Scope**: `maekon-core` (consent Tier 13, `analysis.memory_vault` config, `MemoryVaultWriterPort`), `maekon-analysis` (writer 구현, exporter 재사용), `maekon-storage` (`vault_mirror_state` migration, §1.4), `maekon-web` (설정 표면, erase 오케스트레이터 Phase-3), `src-tauri` (스케줄러 배선, IPC, erase 오케스트레이터 Phase-3)
**Related**: ADR-023 (memory-graph 기반 + digest exporter), ADR-032 (generation-input 계약 — 본 ADR과 나란히 서는 노출 분류, 종속 아님), ADR-028 §P3 (sanitizer floor 선례), ADR-026 (`ConsentManagerPort`), #4478/#4479 (삭제 그림자-사본 사고 클래스 + 수정 패턴), #8056 (Art.20 롤업), #9465 (구현 이슈)
**Issue**: #9465 (MK-MEM-01.T03)

---

## 배경 (Context)

memory graph는 내구성 있는 활동 유래 claim을 축적하며(ADR-023), 오늘 사용자가 그것을 자기 손에 쥘 유일한 방법은 `GET /api/digests/daily/export` — `DigestExporter::to_markdown_with_claims`(`crates/maekon-core/src/models/daily_digest.rs:150`; exporter는 `Retracted`를 내부 배제하고, `Superseded`까지 배제하는 `Active`-only 선택은 호출자 — `handlers/daily_digest.rs:76-85` — 에 산다)로 렌더되는 1회성 다운로드다. MK-MEM-01(#9462)은 다음 단계로 **지속 미러링되는 로컬 마크다운 볼트**를 원한다. 가장 근접한 비교 제품의 실사용 리뷰들은 inspectable vault를 단일 최고 호평 속성으로 지목한다.

이것이 기능 PR이 아니라 아키텍처 결정인 이유는 *SQLite 밖의 지속 평문 파일 트리*가 갖는 위험 비대칭이다:

- **활동 유래 텍스트의 제2 사본은 새로운 삭제 표면이다.** #4478 사고 클래스가 정확히 이 형태다. 오늘의 삭제는 **독립 오케스트레이터 2개** — `src-tauri/src/commands/consent.rs::erase_all_local_data`(SQL 페이즈 + `FrameStoragePort` 파일 페이즈, `pending_local_erase` 크래시 복구 하) 와 `crates/maekon-web/src/services/data_web_service.rs::DataCommandService::delete_all_data`(자체 SQL/파일 분리) — 로 분산돼 있고, SQLite 계층의 `delete_all_data_inner`(`maintenance/retention.rs`, #4479가 memory 테이블로 확장한 `ALL_TABLES` 스윕)는 비재진입 erase lock 아래의 순수 SQL 본체다. 어느 것도 사용자 선택 디렉토리의 파일을 모르며, 한쪽 오케스트레이터에만 배선된 볼트는 다른 쪽에서 #4478을 재현한다.
- **클라우드-싱크 대상 폴더는 암묵적 off-device egress다.** ADR-023 최종 감사 결론은 `egress_safe=true`다. 미러를 iCloud/Dropbox/OneDrive/Google Drive 폴더(자연스러운 선택: 기존 Obsidian 볼트)로 지정하면 claim 텍스트가 **egress ledger 기록 0건**으로 지속적으로 기기를 떠난다 — ledger(`EgressLedgerSink::record_egress`, 의도적 erase-잔존·`ALL_TABLES` 비포함)는 유일한 "무엇이 기기를 떠났나" 진실 표면인데 이 경로가 그것을 통째로 우회한다.
- **미러의 파일명이 사용자의 것과 충돌한다.** Obsidian의 Daily Notes 관례가 정확히 `daily/YYYY-MM-DD.md`다. 네이밍 패턴 일치 파일을 무조건 덮어쓰거나 삭제하는 미러는 첫 사이클에 사용자의 기존 일일 노트를 파괴한다 — 대표 유스케이스 안의 예견 가능한 데이터-손실 버그.
- **기존 export는 export 시점 마스킹이 없다.** `export_daily_digest`는 저장 텍스트를 그대로 렌더한다. 사용자가 명시적으로 트리거하는 1회성 다운로드에는 수용 가능하나, 무인 상주하며 서드파티 도구가 인덱싱·싱크·백업할 수 있는 파일에는 다른 계산이 적용된다.
- **Retention 발산.** claim은 retention 윈도우(`analysis.embedding.retention_days`, 기본 90)에서 prune된다. 파일은 결정하지 않으면 그런 수명주기가 없다.

ADR-023은 `Accepted — fully implemented`다. 미구현 선행 표면을 접붙이면 그 상태가 흐려진다(ADR-032와 동일 논리). 따라서 전용 ADR이다.

## 결정 (Decision)

**단방향·재생성 가능·유계 미러**를 fail-closed consent/config 게이트, 오케스트레이터-수준 삭제 전파, 헤더-마커 충돌 가드, 명시적 클라우드-싱크 경계와 함께 채택한다.

### 1. 제품 형태: 두 파일 클래스, 파생 뷰, 아카이브 아님

1. 볼트는 SQLite SSOT의 **파생·재생성 가능 뷰**이며, 기저 데이터의 실제 행태에 맞춘 두 파일 클래스로 구성된다:
   - **일자 파일** — `vault/daily/YYYY-MM-DD.md`, digest 본문만(`DigestExporter::to_markdown`, claims 섹션 없음). 하루의 digest 행은 생성 후 드물게 변한다(후행 LLM-내러티브 백필이 upsert). 따라서 일자 파일은 렌더 내용이 변할 때만 재쓰기되며(§1.4), 그 외에는 만료(§7) 또는 삭제(§4)만 된다.
   - **Claims 파일** — `vault/claims.md`, 현재 `Active` claims를 오늘 `to_markdown_with_claims`가 쓰는 claim 렌더링 로직으로 렌더 — 구현 PR에서 `maekon-core`의 순수 `DigestExporter::claims_to_markdown(&[MemoryClaim])`으로 추출(동일 필드·동일 `Retracted` 배제 불변식 — 렌더링 로직 추출이지 신규 노출 표면이 아님). claims는 전역이며 일자 연관이 없으므로(`MemoryClaim`에 day association 없음), 그래프가 변할 때 변하는 **파일 하나**를 갖는다 — 모든 일자 파일에 복사되지 않는다.
   - 생성되는 `vault/README.md` 인덱스(세 번째 생성 파일, 동일 헤더/마커 규칙).
2. **엄격한 단방향.** 제품은 볼트 파일 *내용*을 결코 읽지 않는다(§6.4 충돌 가드는 제품 헤더 검증에 필요한 만큼만 읽는다). 사용자 편집은 병합되지 않고 재생성 시 덮어써진다. 모든 생성 파일은 정확히 그 사실을 밝히는 고정 제품 헤더 줄(마커, §6.4)로 시작한다. 파일 감시는 없다.
3. **볼트는 아카이브가 아니다.** 현재 DB 진실을 미러한다: 윈도우 밖 일자 파일은 재생성 시 만료되고(§7.3), `claims.md`는 항상 현재 `Active` 집합을 반영한다(prune/retract/supersede된 claim은 다음 사이클에 사라진다). 장기 보관은 사용자의 행위 — 볼트 밖으로 복사 — 이며 그 순간 사본은 제품 책임 밖의 사용자 소유 데이터다(20조 이동권 시멘틱, #8056 부분 이행).
4. **경계와 변경 감지**: `mirror_window_days`(기본 **90**, ≥ 1 그리고 ≤ `analysis.embedding.retention_days`)가 관리 파일을 윈도우 + 2로 제한한다. 파일은 새로 렌더된 내용이 **저장된 경로별 콘텐츠 해시**와 다르거나, 해시 행이 없거나, **파일 자체가 디스크에 없을 때** (재)쓰기된다 — 사이클별 존재 검사: 사용자가 삭제한 파일은 재생성된다(재생성-가능-뷰 약속 보존; 해시-only 비교가 소실 파일의 재생성을 억제해서는 안 된다). 해시는 신규 **`vault_mirror_state`** SQLite 테이블(`maekon-storage` migration; 경로 키 행에 마지막-렌더 콘텐츠 해시 — 파일 내용 read-back 없음, §1.2)에 산다. 이 테이블은 삭제 `ALL_TABLES` 스윕에 합류한다 — 해시 상태가 자기가 기술하는 파일보다 오래 살 수 없다(erase를 생존한 고아 해시 행은 재생성을 침묵 억제하게 된다); 만료 스윕이 지운 파일의 행도 함께 제거된다. 조용한 날은 쓰기 0; claims 변경은 정확히 `claims.md`만 건드린다.
5. **경계 위반은 평가 불가 게이트다** — `MemoryGraphProjectionConfig` 형제와 정확히 동일한 의미론: `mirror_window_days`가 0이거나 `analysis.embedding.retention_days`를 초과하면(사용자가 나중에 retention을 이미 설정된 윈도우 아래로 낮춘 경우 포함) 사이클은 **완전 no-op** — 쓰기 없음 그리고 삭제 없음(잘못된 설정이 볼트를 삭제해서는 안 된다), debug 로그. clamp 없음, 확장 없음.

### 2. Consent와 설정 (fail-closed)

1. 신규 전용 `ConsentPermissions.memory_vault_mirror` — **Tier 13**(Tier 11/12는 ADR-032가 Mode B/C 이름으로 예약), `#[serde(default)]` false, 본 ADR 인용 doc comment, 어떤 형제 권한에서도 빌리거나 함의되지 않음.
2. 신규 config 섹션 `analysis.memory_vault`(`MemoryVaultConfig`): `enabled`(기본 false), `custom_path: Option<PathBuf>`(기본 `None` = 앱 소유 기본 위치), `custom_path_acknowledged: bool`(기본 false, §3.3), `cloud_provider: Option<String>`(수락 시점 감지 결과 저장, §3.2), `mirror_window_days`(기본 90).
3. writer는 `enabled` AND `memory_vault_mirror` consent AND consent `deletion_flag` 해제로 게이트된다 — 모든 SQLite writer의 skip-while-erasing 규율과 동일. 평가 불가 게이트 — `data_dir()` 해석 실패(`config_manager::data_dir`는 `Result` 반환) 포함 — 는 완전 no-op 사이클(fail-closed; ADR-032 §2 의미론의 거울).

### 3. 커스텀-경로 경계 (하중 조항)

1. **기본 위치**: `<data_dir()>/vault` — 앱 소유, 플랫폼 로컬, 어떤 클라우드-싱크 루트 하위도 아님. 기본 위치는 §2 게이트만 요구한다.
2. **클라우드-싱크 감지**는 canonicalize된 대상에 대해 **경로 수락 시점에 1회** 실행되고 결과가 `cloud_provider`(§2.2)로 저장된다 — 사이클별 진실은 라이브 재감지가 아니라 저장값이다:
   - macOS: `~/Library/Mobile Documents/`(iCloud Drive) 또는 `~/Library/CloudStorage/`(Dropbox/Google Drive/OneDrive/Box provider 폴더의 OS 마운트 지점) 하위.
   - Windows: `%OneDrive%`/`%OneDriveCommercial%` 하위, 또는 알려진 provider 루트(`~/Dropbox`, `~/Google Drive`).
   - Linux: best-effort 알려진 루트.
   감지의 역할은 의도적으로 좁다: **§3.3 경고 카피를** 명명된 provider로 **강화**하고 **§3.4 ledger 기록을 게이트**한다. 그 자체로는 어떤 consent 결정도 게이트하지 않는다.
3. **모든 커스텀 경로 — 감지 여부 무관 — 는 별도 승인을 요구한다**(`custom_path_acknowledged = true`, 명시적 UI 플로우). 그 카피는 두 위험을 평이하게 밝힌다: (a) 그 폴더에서 미러 네이밍 패턴에 맞는 파일은 병합 없이 덮어쓰기/삭제된다(§6.4 마커 가드만이 예외), (b) 그 폴더가 어떤 메커니즘으로든 싱크되면 — 감지 시 명명된 provider, 아니면 "당신이 돌리는 어떤 싱크 도구든" — claim 텍스트가 그것을 통해 기기를 떠나며, Maekon은 모든 싱크 메커니즘을 감지할 수 없다. 승인 없으면 커스텀 경로는 거부되고 미러는 기본 위치에 남는다. (Proposed 초안의 감지/미감지 이원 구조는 폐기 — 무조건 단일 승인이, 목록에 없는 provider가 아무 경고도 못 받던 거짓-확신 공백을 닫는다.)
4. **Egress-ledger 가시성**: 저장된 `cloud_provider`가 설정된 경로에 파일을 하나라도 쓴 재생성 사이클마다 `EgressLedgerRecord` **1건**을 기록하며, 전 필드가 고정된다:
   - `event_type`: `vault_mirror_cloud_sync`
   - `destination`: coarse provider 라벨만(`icloud` | `cloud_storage` | `onedrive` | `dropbox` | `google_drive`) — **파일시스템 경로 절대 불가**: erase-잔존·의도적 no-PII 테이블에 OS 사용자명을 박게 된다(`destination` 자체 doc comment: endpoint 상세는 의도적으로 미기록)
   - `record_id`: `EgressLedgerSink` dedup 관례대로 결정론적 유도 — `vault_mirror|<destination>|<local YYYY-MM-DD>` — replay와 하루 내 다중 사이클이 단일 감사 행으로 수렴(행의 의미: "이 날짜에 볼트가 클라우드-싱크 경로로 미러됨")
   - `byte_count`: 기록을 만든 사이클의 총 쓰기 바이트; `recipient_count`: 1; 나머지 필드는 기존 producer 관례.
   기본 위치 쓰기는 기록하지 않는다 — SQLite 저장소와 같은 device-local이다. ledger 쓰기는 포트 자체의 non-fatal 규율을 따른다(ledger 실패는 log-and-continue; 미러 쓰기는 차단되지 않는다).
5. **정직성 경계**: 감지는 best-effort다. §3.3 카피와 문서는 커스텀 경로가 사용자 책임임을 평이하게 밝힌다. 계약은 *기본* 경로를 절대적으로 방어하고, *모든* 커스텀 경로를 명시적으로 표면화하며, 감지 가능한 것을 강화한다 — 그 이상을 가장하지 않는다.

### 4. 17조 삭제 전파 (#4478 조항)

1. **계층**: 볼트 삭제는 **두 erase 오케스트레이터의 공유 Phase-3**이지, SQLite 계층 `delete_all_data_inner`(파일시스템 I/O를 얻어서는 안 되는, 비재진입 erase lock 아래 순수 SQL 본체)의 확장이 아니다. 단일 구현은 writer와 같은 `maekon-core` 포트 뒤에 살고(`MemoryVaultWriterPort::erase_generated_files`, §7.4), **양쪽** 오케스트레이터가 호출해야 한다:
   - `src-tauri/src/commands/consent.rs::erase_all_local_data` — 기존 `FrameStoragePort` 파일 페이즈와 나란한 신규 페이즈로, `pending_local_erase` 크래시-복구 봉투 안에서(페이즈 사이 크래시는 복구 시 볼트 삭제 재실행);
   - `crates/maekon-web/src/services/data_web_service.rs::DataCommandService::delete_all_data` — 동일 신규 페이즈로.
   오케스트레이터별 계약 테스트(공유 테스트 1개가 아니라)가 의무다 — 두 사이트는 이미 중복된 코드이며, 그것이 정확히 #4478을 낳은 조건이다.
2. **범위**: 기본 볼트 디렉토리는 항상 삭제; 설정된 `custom_path`는 **마커 보유 생성 파일만** 제거(§6.4 — 사용자 자신의 노트가 있을 수 있는 폴더의 재귀 삭제는 절대 아님).
3. **실패는 표면화되고 삼켜지지 않는다**: `erase_generated_files`는 파일별 리포트를 반환한다; 실패를 받은 오케스트레이터는 자기 결과에 반영해야 한다(삭제 미완 보고, 가능한 곳에서 크래시-복구 봉투로 재시도) — `warn!`-and-continue 절대 불가. 공개된 claim 텍스트를 담은 파일은 컴플라이언스 표면이지, frame 썸네일 같은 best-effort 자산 클래스가 아니다.
4. **#4479 패턴 regression guard**(fail-before/pass-after)가 구현 PR에 의무다: claim 시드 → 미러 사이클 → erase → 전 생성 파일(일자·`claims.md`·인덱스) 소멸 단언, **양쪽 오케스트레이터 경로 모두에서**.
5. erase-배리어 순서(deletion flag 선설정 + §2.3 writer 게이트)가 재생성이 erase 중간/이후에 착지하지 않음을 보장한다.
6. **무엇이 살아남는지 명시**: 사용자가 볼트 밖으로 복사한 파일(§1.3), 그리고 `vault_mirror_cloud_sync` ledger 행 — egress ledger는 설계상 erase-잔존이며, 그 행은 §3.4의 coarse 라벨만 담는다.

### 5. 콘텐츠와 마스킹 floor

1. 미러 콘텐츠는 §1.1대로 재분할된 기존 export 표면이다: 일자 파일은 digest 본문, `claims.md`는 `Active` claims. **`Active`-only 선택(`list_claims_by_status(ClaimStatus::Active)`)은 writer 자신의 계약 의무다** — 호출부가 아니라 `MemoryVaultWriterPort` 구현 안에 살며, `Superseded`/`Retracted` 텍스트가 생성 파일에 결코 도달할 수 없음을 단언하는 계약 테스트를 갖는다. (오늘 그 선택은 maekon-web export 핸들러에만 존재한다; writer는 다른 crate의 새 call site이며 더 넓은 claims 쿼리에 배선되어서는 안 된다.)
2. **엔드포인트보다 엄격한, 전체-문서 sanitizer floor**: 모든 생성 파일의 **렌더된 마크다운 전체** — digest 내러티브·하이라이트·타임라인 항목·claim 텍스트 — 가 주입된 `PiiSanitizer`를 `PiiFilterLevel::Standard` 최소로, **렌더 후 문서 전체에 1회** 통과한 뒤 원자적 쓰기된다. Fail-closed: sanitizer 미배선 ⇒ 볼트 쓰기 없음(ADR-028 §P3 선례). 렌더-후 적용이 floor가 claims 부록만이 아니라 digest 본문(공유 `render_body` 렌더)까지 커버하게 만드는 방법이다. 1회성 HTTP export보다 의도적으로 엄격하며, 델타는 문서화하고 엔드포인트 정렬은 별도 결정이다.
3. Retraction 가시성: retract된 claim은 다음 재생성 사이클에서 `claims.md`에서 사라진다; 경성 보장은 다음 일일 사이클. (retract 핸들러발 즉시 재생성의 교차-crate 배선은 계약 조항이 아니라 Known Follow-up이다 — 두 crate 사이에 스케줄러-트리거 프리미티브가 현재 없다.)

### 6. 파일시스템 안전

1. **원자적 쓰기**: 파일별 temp 파일 + 동일 디렉토리 rename.
2. **격리**: 볼트 루트는 사이클당 1회 canonicalize; 모든 쓰기·삭제는 대상이 그 루트 하위로 resolve됨을 재검증한다(symlink 탈출은 거부) — `data_web_service.rs`의 기존 canonicalize + `starts_with` 패턴이 모델이다.
3. **유계**: 관리 파일 최대 윈도우 + 2; writer는 자기 네이밍 패턴 밖의 어떤 것도 열거하거나 만지지 않는다.
4. **마커 가드 (충돌 안전)**: 모든 생성 파일은 고정 제품 헤더 줄(마커)로 시작한다. writer는 마커 없는 패턴-일치 파일을 **절대 덮어쓰지 않고**, writer/eraser는 **절대 삭제하지 않는다** — 그런 파일은 건너뛰고, 사이클 리포트에 충돌로 집계되고, 설정/상태 UI에 표면화된다; 사용자가 그 파일을 제거/개명할 때까지 미러는 그 파일명을 채택하지 않는다. 이것이 `custom_path`를 살아 있는 Obsidian 볼트로 지정해도 안전한 이유다: 기존 사용자 일일 노트는 마커가 없어 구조적으로 불가침이다. (검증은 헤더 접두부만 읽는다 — §1.2의 명시 carve-out이지 내용 read-back이 아니다.)

**개정 (#9522, 2026-07-30) — "설정/상태 UI에 표면화" 조항은 사이클 단위이지 호출 단위가 아니다.** 사이클 리포트는 그 사이클을 호출한 주체에게만 반환되므로, 최초 구현에서는 **수동** "Export now"의 충돌만 보였다; 대표 시나리오 — *스케줄* 사이클이 기존 Obsidian 일일 노트를 조용히 건너뛰는 경우 — 는 사용자가 우연히 그 버튼을 누를 때까지 보이지 않았다. 따라서 마지막으로 실행된 사이클은 요약(시각, 기록·만료 개수, 그리고 충돌은 **볼트-상대 경로명만**, 상한 적용)을 `vault_mirror_state`의 `::` 접두 예약 키에 영속하고, §3 설정 payload와 함께 읽어 온다. 예약 행은 파일명이 아니며 해시 행과 동일한 §4 `ALL_TABLES` 삭제 스윕을 타므로, Art.17을 생존할 수 있는 상태를 새로 만들지 않는다. fail-closed no-op 사이클은 의도적으로 기록하지 **않는다** — 빈 "기능 비활성" 기록은 이 조항이 보여주려는 충돌 리포트를 파괴하기 때문이다.

### 7. 사이클 정의와 스케줄링

**미러 사이클**은 `MemoryVaultWriterPort::run_mirror_cycle` 1회 호출이며 순서대로 수행한다:

1. **일자-파일 채움**: 미러 윈도우 내 각 날짜에 대해 digest 행이 존재하고(`DigestStorage` 포트로 읽음 — `crates/maekon-core/src/ports/web_storage.rs:288`) §1.4 staleness 조건이 성립하면(해시 없음·해시 stale·파일 디스크 소실) 일자 파일을 렌더·쓰기한다. 이것이 digest catch-up seam을 포섭한다: 스케줄러의 기존 일일 digest 생성(`scheduler/loops/system.rs` aggregation 경로)이 먼저 돌고 forward-only다(`daily_catchup_dates`는 기존 digest에서 short-circuit). 따라서 사이클은 생성 루프의 제어 흐름에 편승하는 대신 digest **행**을 읽는다 — Proposed 초안의 "루프 편승" 문구는 파일 만료·재방문을 구조적으로 할 수 없었다.
2. **Claims-파일 재생성**: writer 자신의 `Active` 선택(§5.1)으로 `claims.md` 렌더; 동일한 §1.4 조건(해시 변경 또는 파일 소실)에서 재쓰기.
3. **만료 스윕**: canonical 루트 하위의 마커 보유 생성 파일을 열거; 날짜가 윈도우 밖인 것을 삭제(마커 + 패턴 + 격리 검사 전부 적용, §6).
4. 포트(`maekon-core`, 구현 `maekon-analysis`, DI 배선 `src-tauri` — ADR-032 배치 패턴):

```rust
#[async_trait]
pub trait MemoryVaultWriterPort: Send + Sync {
    /// One full mirror cycle (§7.1–§7.3). Fail-closed: any unevaluable
    /// §2 gate or §1.5 bound violation yields a no-op Ok with the reason
    /// in the stats. Storage errors propagate as Err.
    async fn run_mirror_cycle(&self, now_secs: i64) -> Result<VaultCycleStats, CoreError>;

    /// Art.17 Phase-3 (§4): delete every marker-bearing generated file
    /// under the active vault root. Per-file failures are reported in the
    /// result, never swallowed.
    async fn erase_generated_files(&self) -> Result<VaultEraseReport, CoreError>;
}
```

   구현은 주입된 core 포트들(`DigestStorage`(digest 행), `MemoryGraphPort`(claims), `PiiSanitizer`, `EgressLedgerSink`, `ConsentManagerPort`, `ConfigManager`)로 자기 입력을 스스로 가져온다 — 스케줄러는 `now_secs`만 넘긴다.
5. **트리거**: 스케줄러가 일일 digest 생성 완료 후 사이클 1회 호출; "Export now" IPC가 사이클 1회 호출(오늘-만 export가 아니라 전체 §7.1–§7.3 사이클 — 구현 PR의 명명은 구 1회성 시멘틱을 암시하지 않아야 한다).

## 결과 (Consequences)

### 긍정
- 비교 제품의 최고 호평 속성이 이미 감사된 exporter 위의 얇은 층으로 착지한다 — 기존 엔드포인트와 동일 필드, at-rest floor는 더 엄격.
- 이 표면이 도입할 수 있는 세 침묵-실패 클래스(중복 오케스트레이터를 가로지르는 삭제 그림자 사본, ledger 없는 클라우드 egress, 사용자 볼트 내 충돌 데이터-손실)가 구현 *전에* 명명·게이트·테스트 의무화된다.
- Art.20 이동권이 무답 대신 상시 부분 답을 얻는다(#8056).

### 부정
- 클라우드-싱크 감지는 OS별 유지보수 표면으로 남되, 경고-강화 + ledger-게이팅으로 강등된다(drift가 consent 플로우에 더는 영향 없음).
- 볼트-대-엔드포인트 마스킹 델타(§5.2)는 엔드포인트 결정 전까지 문서가 짊어질 의도적 비일관성이다.
- 단방향 재생성은 *마커 보유* 파일의 사용자 편집을 헤더 경고에도 불구하고 덮어쓴다 — SSOT 무결성을 위해 수용한 트레이드(마커 없는 파일은 구조적으로 안전, §6.4).

### 중립
- 구현 PR 전 런타임 변화 없음; 기본값은 기능을 완전히 끈다(consent false AND enabled false).
- 볼트는 사용자 선택의 onward 노출을 쉽게 만든다(그것이 목적); ADR-032는 *Maekon 자신의* 생성 파이프라인이 읽을 수 있는 것을 계속 규율한다 — 두 경계는 설계상 서로소다.

## 검토한 대안 (Alternatives Considered)

**A. ADR-023 개정.** 기각 — ADR-032와 동일한 상태-모호성 논리; cross-cutting 표면이지 기반 변경이 아니다.

**B. 암호화 볼트.** 기각 — 목적을 무효화한다(일반 에디터에서 열람 가능한 파일이 기능이다).

**C. 양방향 싱크.** 기각 — 표시 표면을 memory graph로의 비인증 쓰기 경로로 바꾼다(ADR-032 소비자에 공급되는 저장소로의 untrusted-content 주입). 단방향은 단계가 아니라 계약이다.

**D. 무한 아카이브.** 기각 — 활동 유래 텍스트의 무한 평문 축적은 SSOT를 제한하는 retention 자세(GDPR Art. 5(1)(e))와 모순된다; 보관은 사용자의 명시적 행위다(§1.3).

**E. 파일 쓰기당 ledger 기록.** 기각 — 일자별 dedup granularity(§3.4)가 ledger를 신호 밀도 있게 유지한다.

**F. 모든 일자 파일에 claims 섹션 (Proposed 초안 형태).** 리뷰로 기각 — claims는 전역이라 일자별 중복은 ~윈도우 개의 동일 사본을 렌더하고, §1.4 변경 감지를 깨며(claims 1건 변경이 전 파일을 dirty), "어느 날의 claims인가"가 미정의였다. 두 파일 클래스가 데이터의 실제 형태에 맞는다.

## 알려진 후속 작업 (Known Follow-ups)

1. **구현 PR**(#9465): `MemoryVaultConfig` + Tier-13 consent + `MemoryVaultWriterPort`(§7.4) + `DigestExporter::claims_to_markdown` 추출 + writer 구현(`maekon-analysis`) + **양쪽** 오케스트레이터 Phase-3 삭제 + §4.4 regression guard(양 경로) + §6.4 마커-가드 테스트 + 설정 표면(`maekon-web`) + 스케줄러/IPC 배선(`src-tauri`). §4/§6 테스트는 제안이 아니라 인수 기준이다.
2. **클라우드-감지 테이블 유지보수** — §3.2 목록은 drift한다; 추가는 docs+const 갱신으로.
3. **HTTP export 마스킹 정렬** — 별도 결정.
4. **Art.20 전체 export**(#8056 잔여) — 열려 있다.
5. **Retract-트리거 즉시 재생성** — 오늘 존재하지 않는 교차-crate 스케줄러-트리거 프리미티브가 필요하다; 원한다면 ad-hoc 결합이 아니라 자체 소형 seam으로 설계할 것(§5.3).

## 개정 이력 (Amendment History)

- **2026-07-29 (#9465, 3-loop 리뷰: devils-advocate + 구현자 렌즈; BLOCKING/IMPORTANT 전량 반영):**
  1. **두 파일 클래스**(§1.1, 대안 F) — claims를 일자 파일에서 단일 `claims.md`로 분리; 일자 파일은 digest 본문만. 전역-claims/일자-파일 모순(~91개 파일의 동일 claims 섹션, claims 1건 변경 시 전 파일 dirty) 해소.
  2. **사이클을 catch-up 루프에서 분리 재정의**(§7) — Proposed의 "일일 digest 루프 편승"은 구조적으로 파일 만료·재방문 불가(`daily_catchup_dates`는 forward-only + 기존 digest short-circuit); 사이클은 이제 `DigestStorage`로 digest 행을 읽고 채움/재생성/만료를 명시적으로 소유한다.
  3. **포트 고정**(§7.4) — `MemoryVaultWriterPort` 시그니처, 입력 소유권(writer가 주입된 core 포트로 스스로 fetch), ADR-032 배치 패턴.
  4. **삭제 재계층화**(§4) — locked SQL 본체 밖으로, 이미 중복된 **양쪽** erase 오케스트레이터(명명됨)가 호출하는 공유 Phase-3로; 오케스트레이터별 계약 테스트, 표면화되는(삼켜지지 않는) 실패, 크래시-복구 참여.
  5. **마커 가드**(§6.4) — 생성-파일 헤더 마커; 마커 없는 파일은 절대 덮어쓰기/삭제 불가. Obsidian daily-notes 충돌(대표 유스케이스 내 데이터 손실) 봉쇄.
  6. **커스텀-경로 승인 무조건화**(§3.3) — 감지/미감지 이원 구조 제거; 감지는 경고-강화 + ledger-게이팅으로 강등, 수락 시점 1회 실행·저장(`cloud_provider`).
  7. **Ledger 기록 전체 고정**(§3.4) — coarse `destination` 라벨(경로 절대 불가 — erase-잔존 no-PII 테이블), 결정론적 `record_id` watermark(`vault_mirror|<destination>|<date>`), `byte_count`/`recipient_count` 시멘틱, non-fatal ledger-실패 규율.
  8. **Active-only 선택을 writer 계약 의무화**(§5.1) + no-`Superseded`/`Retracted`-텍스트 계약 테스트; **sanitizer floor를 렌더-후 전체 문서로 확장**(§5.2) — digest 내러티브/하이라이트/타임라인까지 커버.
  9. **경계-위반 의미론 고정**(§1.5) — 평가 불가 게이트 no-op(쓰기 없음 그리고 삭제 없음), clamp 없음; `data_dir()` 가류성은 §2.3에 편입.
  10. Minors: ledger 행의 erasure 생존 명시(§4.6); "추가 노출 없음" 문구를 동일-필드로 정밀화; "Export now" 범위 = 전체 사이클(§7.5); 감지 주기 = 수락 시 1회·저장값이 사이클 진실(§3.2); retract-즉시 배선은 Known Follow-up 5로 이동.
- **2026-07-29, 2차 확인 패스 (구현자 렌즈, 개정 자신의 해시 메커니즘에 IMPORTANT 2 + MINOR 1):**
  11. **해시 저장 위치 고정**(§1.4) — "config-state/SQLite" 얼버무림을 명명된 `vault_mirror_state` SQLite 테이블(`maekon-storage` migration, Scope 추가)로 대체; 삭제 `ALL_TABLES` 스윕 멤버라 해시 상태가 자기가 기술하는 파일보다 오래 살 수 없다.
  12. **소실-파일 자가치유 복원**(§1.4/§7.1/§7.2) — staleness 조건은 해시-없음 OR 해시-stale OR **파일 디스크 소실**; 저장-해시 일치가 삭제된 파일의 재생성을 억제해서는 안 된다(구 byte-compare 설계가 암묵적으로 갖던 속성).
  13. §1.1 일자-파일 불변성 문구를 §7.1과 정렬(digest 행은 upsert된다 — LLM-내러티브 백필 등 — 일자 파일은 "한 번 쓰기"가 아니라 렌더-내용 변경 시 재쓰기).

## 관련 문서 (Related Docs)

- `docs/architecture/ADR-023-local-symbolic-memory-graph.ko.md` — 기반, exporter, 삭제 이력
- `docs/architecture/ADR-032-memory-graph-generation-input-contract.ko.md` — 형제 경계(파이프라인 측 노출)
- `docs/architecture/ADR-028-durable-task-lifecycle-boundary.ko.md` §P3 — sanitizer floor 선례
- `crates/maekon-core/src/models/daily_digest.rs` — `DigestExporter`(`to_markdown`, `to_markdown_with_claims`)
- `crates/maekon-core/src/ports/web_storage.rs` — `DigestStorage`(§7.1 읽기 seam)
- `crates/maekon-storage/src/sqlite/maintenance/retention.rs` — `delete_all_data_inner` / `ALL_TABLES`(§4가 의도적으로 확장하지 않는 계층)
- `src-tauri/src/commands/consent.rs` / `crates/maekon-web/src/services/data_web_service.rs` — §4.1이 구속하는 두 erase 오케스트레이터
- `crates/maekon-core/src/ports/egress_ledger.rs` — `EgressLedgerSink`(§3.4)
