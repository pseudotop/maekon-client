[English](./oneshim-web.md) | [한국어](./oneshim-web.ko.md)

# oneshim-web

로컬 웹 대시보드 크레이트. Axum 0.8 기반 REST API와 React 18.3 프론트엔드를 제공합니다.

## 기능

- **REST API**: Axum 기반 API 표면 (메트릭, 프로세스, 프레임, 이벤트, 태그, 리포트, 자동화 등)
- **실시간 SSE**: Server-Sent Events 스트림 (메트릭, 프레임, 유휴 상태)
- **React 프론트엔드**: rust-embed로 바이너리에 임베드
- **자동 포트 찾기**: 포트 충돌 시 다음 포트 자동 시도
- **자동화 대시보드**: 자동화 상태, 감사 로그, 워크플로우 프리셋, 실행 통계

## 구조

```
oneshim-web/
├── src/
│   ├── lib.rs          # WebServer + AppState (audit_logger 포함)
│   ├── routes.rs       # 118개 라우트 정의 (368 LoC) — HTTP 표면의 기준 소스;
│   │                   # docs/contracts/oneshim-web.v1.openapi.yaml +
│   │                   # http-interface-manifest.v1.json 로 contract freeze
│   ├── error.rs        # ApiError 타입
│   ├── embedded.rs     # 정적 파일 서빙 (rust-embed)
│   └── handlers/       # 44개 API 핸들러 파일 (플랫 + 도메인 그룹):
│       # 원래 17개 (아래 Phase-1 베이스라인). Superpowers + Phase-4로 확장.
│       # 도메인 그룹 서브디렉토리 + 플랫 파일:
│       # ai_models, ai_provider_surfaces, ai_session, annotations, automation,
│       # automation_gui, backup, bug_report, coaching, daily_digest, dashboard,
│       # data, digests, events, export, focus, frames, idle, integration,
│       # metrics, onboarding, playbooks, pomodoro, processes, recalibration,
│       # reports, search, sessions, settings, stats, stream, suggestions,
│       # support, tags, timeline, update + automation/ 서브디렉토리
│       # (mod.rs + helpers.rs + scene.rs + execution.rs).
└── frontend/           # React 18.3 프론트엔드
    ├── src/
    │   ├── pages/      # 페이지 컴포넌트 (Dashboard, Automation, Settings 등)
    │   ├── components/ # UI 컴포넌트
    │   ├── api/        # API 클라이언트 (ADR-019 IpcError TS type guard 포함한 desktop.ts)
    │   ├── hooks/      # React 훅
    │   ├── i18n/       # i18n 번역 (en/ko) + wire-errors.{en,ko}.json (41 code)
    │   └── styles/     # 디자인 토큰
    ├── e2e/            # Playwright E2E 테스트
    └── # Biome (lint), Vitest (unit), Playwright (e2e), Storybook (review catalog)
```

## AppState

```rust
#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn WebStorage>,
    pub frames_dir: Option<PathBuf>,
    pub event_tx: broadcast::Sender<RealtimeEvent>,
    pub config_manager: Option<ConfigManager>,
    pub audit_logger: Option<Arc<RwLock<AuditLogger>>>,
    pub automation_controller: Option<Arc<AutomationController>>,
    pub update_control: Option<UpdateControl>,
}
```

### WebServer 빌더

```rust
let server = WebServer::new(storage, web_config)
    .with_config_manager(config_manager)
    .with_audit_logger(audit_logger)
    .with_automation_controller(automation_controller)
    .with_update_control(update_control)
    .with_event_tx(event_tx)
    .with_frames_dir(frames_dir);

server.run(shutdown_rx).await?;
```

### 저장소 포트 (`storage_port.rs`)

`oneshim-web` 핸들러는 구체 구현체 `SqliteStorage`가 아니라 포트 `WebStorage`에 의존한다.

- 포트 정의: `WebStorage: StorageService + MetricsStorage + Send + Sync`
- 포함 범위: 프레임/이벤트 조회, 검색, 태그, 집중도 메트릭/제안, 백업/내보내기, 범위 삭제
- 현재 어댑터: `impl WebStorage for SqliteStorage`

이 구조로 웹 애플리케이션 계층은 저장소 구현 교체 시에도 핸들러 코드를 바꾸지 않는다.

## API 엔드포인트

정규 API 인벤토리 기준:
- `docs/contracts/http-interface-manifest.v1.json`
- `docs/contracts/oneshim-web.v1.openapi.yaml`

아래 표는 빠른 파악을 위한 그룹 예시입니다. 정확한 최신 표면은 위 manifest/OpenAPI 스냅샷을 사용하세요.

### 메트릭
| 메서드 | 경로 | 설명 |
|--------|------|------|
| GET | `/api/metrics` | 최신 시스템 메트릭 |
| GET | `/api/metrics/history` | 메트릭 히스토리 |
| GET | `/api/stats/heatmap` | 활동 히트맵 |

### 프로세스/세션
| 메서드 | 경로 | 설명 |
|--------|------|------|
| GET | `/api/processes` | 현재 프로세스 목록 |
| GET | `/api/idle` | 유휴 기간 목록 |
| GET | `/api/sessions` | 세션 통계 |

### 프레임/이벤트
| 메서드 | 경로 | 설명 |
|--------|------|------|
| GET | `/api/frames` | 프레임 목록 (페이지네이션) |
| GET | `/api/frames/:id` | 프레임 상세 |
| GET | `/api/frames/:id/image` | 프레임 이미지 |
| GET | `/api/events` | 이벤트 목록 |

### 태그
| 메서드 | 경로 | 설명 |
|--------|------|------|
| GET | `/api/tags` | 모든 태그 |
| POST | `/api/tags` | 태그 생성 |
| PUT | `/api/tags/:id` | 태그 수정 |
| DELETE | `/api/tags/:id` | 태그 삭제 |
| POST | `/api/frames/:id/tags/:tag_id` | 프레임에 태그 추가 |
| DELETE | `/api/frames/:id/tags/:tag_id` | 프레임에서 태그 제거 |

### 검색/리포트
| 메서드 | 경로 | 설명 |
|--------|------|------|
| GET | `/api/search` | 통합 검색 |
| GET | `/api/reports` | 활동 리포트 |
| GET | `/api/timeline` | 통합 타임라인 |

### 집중도 분석
| 메서드 | 경로 | 설명 |
|--------|------|------|
| GET | `/api/focus/metrics` | 집중도 메트릭 |
| GET | `/api/focus/sessions` | 작업 세션 |
| GET | `/api/focus/interruptions` | 중단 이벤트 |
| GET | `/api/focus/suggestions` | 로컬 제안 |
| POST | `/api/focus/suggestions/:id/feedback` | 제안 피드백 |

### 설정/백업
| 메서드 | 경로 | 설명 |
|--------|------|------|
| GET | `/api/settings` | 설정 조회 (자동화/샌드박스/AI 포함) |
| POST | `/api/settings` | 설정 변경 |
| GET | `/api/backup` | 백업 생성 |
| POST | `/api/backup/restore` | 백업 복원 |
| GET | `/api/export/:type` | 데이터 내보내기 |

### 자동화 (Automation)
| 메서드 | 경로 | 설명 |
|--------|------|------|
| GET | `/api/automation/status` | 자동화 시스템 상태 |
| GET | `/api/automation/audit` | 감사 로그 조회 (limit, status 필터) |
| GET | `/api/automation/policies` | 활성 정책 요약 |
| GET | `/api/automation/stats` | 실행 통계 (성공/실패/거부/타임아웃) |
| GET | `/api/automation/presets` | 프리셋 목록 (내장 + 사용자) |
| POST | `/api/automation/presets` | 사용자 프리셋 생성 |
| PUT | `/api/automation/presets/:id` | 사용자 프리셋 수정 |
| DELETE | `/api/automation/presets/:id` | 사용자 프리셋 삭제 |
| POST | `/api/automation/presets/:id/run` | 프리셋 실행 |

### 실시간 스트림
| 메서드 | 경로 | 설명 |
|--------|------|------|
| GET | `/api/stream` | SSE 이벤트 스트림 |

## 자동화 API 상세

### DTO 타입

```rust
/// 자동화 시스템 상태
pub struct AutomationStatusDto {
    pub enabled: bool,
    pub sandbox_enabled: bool,
    pub sandbox_profile: String,
    pub ocr_provider: String,
    pub llm_provider: String,
    pub external_data_policy: String,
    pub pending_audit_entries: usize,
    pub scene_action_override_enabled: bool,
    pub scene_action_override_active: bool,
    pub scene_action_override_reason: Option<String>,
    pub scene_action_override_approved_by: Option<String>,
    pub scene_action_override_expires_at: Option<String>,
    pub scene_action_override_issue: Option<String>,
}

/// 감사 로그 엔트리
pub struct AuditEntryDto {
    pub entry_id: String,
    pub timestamp: String,
    pub session_id: String,
    pub command_id: String,
    pub action_type: String,
    pub status: String,  // Started | Completed | Failed | Denied | Timeout
    pub details: Option<String>,
    pub elapsed_ms: Option<u64>,
}

/// 실행 통계
pub struct AutomationStatsDto {
    pub total_executions: usize,
    pub successful: usize,
    pub failed: usize,
    pub denied: usize,
    pub timeout: usize,
    pub avg_elapsed_ms: f64,
    pub success_rate: f64,
    pub blocked_rate: f64,
    pub p95_elapsed_ms: f64,
    pub timing_samples: usize,
}

/// 정책 요약
pub struct PoliciesDto {
    pub automation_enabled: bool,
    pub sandbox_profile: String,
    pub sandbox_enabled: bool,
    pub allow_network: bool,
    pub external_data_policy: String,
    pub scene_action_override_enabled: bool,
    pub scene_action_override_active: bool,
    pub scene_action_override_reason: Option<String>,
    pub scene_action_override_approved_by: Option<String>,
    pub scene_action_override_expires_at: Option<String>,
    pub scene_action_override_issue: Option<String>,
}

/// 프리셋 실행 결과
pub struct PresetRunResult {
    pub preset_id: String,
    pub success: bool,
    pub message: String,
}
```

## Settings DTO (자동화 관련)

`AppSettings`에 3개 자동화 섹션 추가:

```rust
pub struct AppSettings {
    // ... 기존 모니터/비전/알림/프라이버시 설정 ...
    pub automation: AutomationSettings,
    pub sandbox: SandboxSettings,
    pub ai_provider: AiProviderSettings,
}

pub struct AutomationSettings { pub enabled: bool }

pub struct SandboxSettings {
    pub enabled: bool,
    pub profile: String,              // "Permissive" | "Standard" | "Strict"
    pub allowed_read_paths: Vec<String>,
    pub allowed_write_paths: Vec<String>,
    pub allow_network: bool,
    pub max_memory_bytes: u64,
    pub max_cpu_time_ms: u64,
}

pub struct AiProviderSettings {
    pub ocr_provider: String,          // "Local" | "Remote"
    pub llm_provider: String,          // "Local" | "Remote"
    pub external_data_policy: String,  // "PiiFilterStrict" | "PiiFilterStandard" | "AllowFiltered"
    pub fallback_to_local: bool,
    pub ocr_api: Option<ExternalApiSettings>,
    pub llm_api: Option<ExternalApiSettings>,
}

pub struct ExternalApiSettings {
    pub endpoint: String,
    pub api_key_masked: String,        // GET: 마스킹 / POST: 전체 키
    pub model: Option<String>,
    pub timeout_secs: u64,
}
```

### API 키 마스킹

- **GET**: `mask_api_key("sk-1234567890abcdef")` → `"sk...cdef"` (앞 2자 + `...` + 뒤 4자)
- **POST**: 전체 키 수신 시 저장, 마스킹된 값(`is_masked_key()`)이면 기존 키 유지

## 프론트엔드 페이지

| 경로 | 페이지 | 단축키 | 설명 |
|------|--------|--------|------|
| `/` | Dashboard | `D` | 시스템 요약, CPU/Memory 차트, 집중도 |
| `/timeline` | Timeline | `T` | 스크린샷 썸네일 그리드 |
| `/search` | Search | — | 통합 검색 + 태그 필터 |
| `/reports` | Reports | `R` | 활동 리포트 + 통계 |
| `/replay` | Session Replay | — | 세션 리플레이 |
| `/focus` | Focus Analytics | — | 집중도 분석 |
| `/automation` | **Automation** | `A` | 자동화 대시보드 |
| `/settings` | Settings | `S` | 설정 (자동화/샌드박스/AI 포함) |
| `/privacy` | Privacy | `P` | 개인정보 관리 |

### Automation 페이지

5개 패널로 구성 (React Query 기반):

1. **상태 카드** — 활성화 여부, 샌드박스 프로필, OCR/LLM 제공자, 대기 감사 항목
2. **워크플로우 프리셋** — 카테고리별 탭 (생산성/앱 관리/워크플로우/사용자), 프리셋 카드 그리드, 실행/CRUD
3. **실행 통계** — 성공/실패/거부/타임아웃 카운트 + 평균 소요 시간
4. **감사 로그** — 테이블 (시각, 명령ID, 액션, 상태 배지, 소요시간), 상태별 필터, 30초 자동 새로고침
5. **정책 정보** — 현재 적용된 정책 요약

### Settings 페이지 (자동화 섹션)

기존 설정에 3개 섹션 추가:

1. **자동화** — 활성화 토글
2. **샌드박스** — 활성화, 프로필 드롭다운, 네트워크 허용 토글
3. **AI 제공자** — OCR/LLM 타입 선택, 데이터 정책, 폴백 토글, 외부 API 설정 (`type="password"`)

## i18n 지원

한국어/영어 번역 키:
- `automation.*` — 자동화 UI 번역 (40개+)
- `settingsAutomation.*` — 자동화 설정 번역 (26개+)
- 기존 번역 유지 (dashboard, timeline, settings, privacy, search, reports 등)

## 사용법

### 기본 실행

```rust
use oneshim_web::WebServer;

let server = WebServer::new(storage, web_config)
    .with_config_manager(config_manager)
    .with_audit_logger(audit_logger)
    .with_automation_controller(automation_controller)
    .with_update_control(update_control)
    .with_event_tx(event_tx);

server.run(shutdown_rx).await?;
```

### 설정

```toml
[web]
enabled = true
port = 10090
allow_external = false
```

## 프론트엔드 개발

```bash
cd crates/oneshim-web/frontend

# 의존성 설치
pnpm install

# 개발 서버
pnpm dev

# 빌드
pnpm build

# E2E 테스트
pnpm test:e2e
```

## 테스트

- Rust/E2E 상태는 최신 GitHub Actions run 페이지를 기준으로 확인합니다.
- 웹 커버리지는 API 핸들러, 라우트, 에러 핸들링, 자동화 DTO 직렬화, 설정 매핑, Playwright 기반 프론트엔드 E2E 시나리오를 포함합니다.
  - 네비게이션, 대시보드, 타임라인
  - 설정, 개인정보, 검색, 리포트
