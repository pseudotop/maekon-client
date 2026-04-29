[English](./oneshim-core.md) | [한국어](./oneshim-core.ko.md)

# oneshim-core

도메인 모델, 포트 인터페이스, 에러 타입, 설정을 정의하는 핵심 크레이트.

## 역할

- **중심 허브**: 다른 모든 크레이트가 의존하는 기반 레이어
- **계약 정의**: Port trait으로 어댑터 인터페이스 표준화
- **타입 안전성**: 도메인 모델과 에러 타입으로 일관된 데이터 흐름 보장

## 디렉토리 구조

```
oneshim-core/src/
├── lib.rs           # 크레이트 루트, 모듈 재export
├── config/          # AppConfig 및 설정 섹션 — 디렉토리 모듈 (ADR-003)
│   ├── mod.rs       # AppConfig 구조체 + Default + 재export
│   ├── enums.rs     # PiiFilterLevel, Weekday, SandboxProfile, AiAccessMode 등
│   └── sections/    # 37개 설정 섹션 구조체 (도메인별 파일로 분리) + Default 구현 (ADR-003 디렉토리 모듈)
├── config_manager.rs # JSON 기반 설정 파일 관리 + 플랫폼별 경로
├── consent.rs       # ConsentManager, GDPR Article 17/20 준수
├── error.rs         # CoreError enum (thiserror, 38개 변형; ADR-019 typed `code: XxxCode` 필드 포함)
├── error_codes/     # 18개 typed code enum (ADR-019 `define_code_enum!` 매크로로 생성)
├── models/          # 도메인 모델
│   ├── mod.rs
│   ├── suggestion.rs   # Suggestion, SuggestionType, Priority
│   ├── event.rs        # ContextEvent, EventType, InputActivityEvent
│   ├── frame.rs        # CapturedFrame, FrameMetadata, ProcessedFrame
│   ├── context.rs      # ContextPayload, DeviceInfo
│   ├── session.rs      # SessionInfo, SessionStatus
│   ├── system.rs       # SystemMetrics, CpuMetrics, MemoryMetrics
│   ├── telemetry.rs    # Telemetry 관련 모델
│   ├── automation.rs   # AutomationAction, MouseButton
│   └── intent.rs       # AutomationIntent, UiElement, WorkflowPreset
└── ports/           # 포트 인터페이스 (trait)
    ├── mod.rs
    ├── api_client.rs   # ApiClient, SseClient, SseEvent
    ├── storage.rs      # StorageService
    ├── monitor.rs      # SystemMonitor, ProcessMonitor, ActivityMonitor
    ├── vision.rs       # CaptureTrigger, FrameProcessor
    ├── notifier.rs     # DesktopNotifier
    ├── compressor.rs   # Compressor
    ├── element_finder.rs # ElementFinder — UI 요소 탐색
    ├── input_driver.rs   # InputDriver — 마우스/키보드 입력
    ├── ocr_provider.rs   # OcrProvider — OCR 텍스트 인식
    ├── llm_provider.rs   # LlmProvider — LLM 추론
    └── sandbox.rs        # Sandbox — OS 네이티브 샌드박스
```

## 주요 모델

### Suggestion
```rust
pub struct Suggestion {
    pub suggestion_id: String,
    pub suggestion_type: SuggestionType,
    pub content: String,
    pub priority: Priority,
    pub confidence_score: f64,
    pub relevance_score: f64,
    pub is_actionable: bool,
    pub created_at: DateTime<Utc>,
}
```

### ContextPayload
```rust
pub struct ContextPayload {
    pub session_id: String,
    pub events: Vec<ContextEvent>,
    pub system_metrics: SystemMetrics,
    pub device_info: DeviceInfo,
    pub timestamp: DateTime<Utc>,
}
```

### CapturedFrame
```rust
pub struct CapturedFrame {
    pub frame_id: String,
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub captured_at: DateTime<Utc>,
    pub source: FrameSource,
}
```

### 자동화 모델 (Automation)

#### AutomationAction — 저수준 자동화 액션
```rust
pub enum AutomationAction {
    MouseMove { x: f64, y: f64 },
    MouseClick { button: MouseButton, x: f64, y: f64 },
    KeyType { text: String },
    KeyPress { key: String },
    KeyRelease { key: String },
    Hotkey { keys: Vec<String> },
}

pub enum MouseButton { Left, Right, Middle }
```

#### AutomationIntent — 서버→클라이언트 고수준 의도
```rust
pub enum AutomationIntent {
    ClickElement { text: String, role: Option<String>, app_name: Option<String>, button: MouseButton },
    TypeIntoElement { element_text: String, role: Option<String>, text: String },
    ExecuteHotkey { keys: Vec<String> },
    WaitForText { text: String, timeout_ms: u64 },
    ActivateApp { app_name: String },
    Raw(AutomationAction),
}
```

#### UiElement — 화면에서 발견된 UI 요소
```rust
pub struct UiElement {
    pub text: String,
    pub bounds: ElementBounds,
    pub role: Option<String>,
    pub confidence: f64,       // 0.0-1.0
    pub source: FinderSource,  // Ocr | Accessibility | TemplateMatcher
}

pub struct ElementBounds {
    pub x: f64, pub y: f64,
    pub width: f64, pub height: f64,
}
```

#### WorkflowPreset — 워크플로우 프리셋
```rust
pub struct WorkflowPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: PresetCategory,
    pub steps: Vec<WorkflowStep>,
    pub builtin: bool,
    pub platform: Option<String>,
}

pub struct WorkflowStep {
    pub name: String,
    pub intent: AutomationIntent,
    pub delay_ms: u64,
    pub stop_on_failure: bool,
}

pub enum PresetCategory {
    Productivity,    // 생산성
    AppManagement,   // 앱 관리
    Workflow,        // 워크플로우
    Custom,          // 사용자 정의
}
```

#### IntentResult / IntentConfig — 실행 결과 및 설정
```rust
pub struct IntentResult {
    pub success: bool,
    pub element: Option<UiElement>,
    pub verification: Option<VerificationResult>,
    pub retry_count: u32,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

pub struct IntentConfig {
    pub min_confidence: f64,        // 0.7
    pub max_retries: u32,           // 3
    pub retry_interval_ms: u64,     // 500
    pub verify_after_action: bool,  // true
    pub verify_delay_ms: u64,       // 1000
}
```

## 포트 인터페이스

### ApiClient
```rust
#[async_trait]
pub trait ApiClient: Send + Sync {
    async fn upload_context(&self, payload: &ContextPayload) -> Result<(), CoreError>;
    async fn upload_frames(&self, frames: Vec<ProcessedFrame>) -> Result<(), CoreError>;
    async fn send_feedback(&self, suggestion_id: &str, accepted: bool) -> Result<(), CoreError>;
}
```

### SseClient
```rust
#[async_trait]
pub trait SseClient: Send + Sync {
    async fn connect(&self, session_id: &str, tx: mpsc::Sender<SseEvent>) -> Result<(), CoreError>;
}
```

### StorageService
```rust
#[async_trait]
pub trait StorageService: Send + Sync {
    async fn save_event(&self, event: &ContextEvent) -> Result<(), CoreError>;
    async fn get_events(&self, since: DateTime<Utc>) -> Result<Vec<ContextEvent>, CoreError>;
    async fn save_frame(&self, frame: &ProcessedFrame) -> Result<(), CoreError>;
    async fn get_frames(&self, since: DateTime<Utc>) -> Result<Vec<ProcessedFrame>, CoreError>;
    async fn cleanup_old_data(&self, before: DateTime<Utc>) -> Result<usize, CoreError>;
}
```

### SystemMonitor
```rust
#[async_trait]
pub trait SystemMonitor: Send + Sync {
    async fn get_metrics(&self) -> Result<SystemMetrics, CoreError>;
}
```

### CaptureTrigger / FrameProcessor
```rust
#[async_trait]
pub trait CaptureTrigger: Send + Sync {
    async fn should_capture(&self, event: &ContextEvent) -> Result<CaptureDecision, CoreError>;
}

#[async_trait]
pub trait FrameProcessor: Send + Sync {
    async fn process(&self, frame: CapturedFrame) -> Result<ProcessedFrame, CoreError>;
}
```

### 자동화 포트 (Automation Ports)

#### Sandbox — OS 네이티브 커널 샌드박스
```rust
#[async_trait]
pub trait Sandbox: Send + Sync {
    async fn execute(&self, action: &AutomationAction) -> Result<(), CoreError>;
    fn capabilities(&self) -> SandboxCapabilities;
}

pub struct SandboxCapabilities {
    pub filesystem_isolation: bool,
    pub network_filtering: bool,
    pub memory_limits: bool,
    pub cpu_limits: bool,
}
```

#### ElementFinder — UI 요소 탐색
```rust
#[async_trait]
pub trait ElementFinder: Send + Sync {
    async fn find_element(&self, text: &str, role: Option<&str>) -> Result<Vec<UiElement>, CoreError>;
}
```

#### InputDriver — 마우스/키보드 입력
```rust
#[async_trait]
pub trait InputDriver: Send + Sync {
    async fn execute_action(&self, action: &AutomationAction) -> Result<(), CoreError>;
}
```

#### OcrProvider — OCR 텍스트 인식
```rust
#[async_trait]
pub trait OcrProvider: Send + Sync {
    async fn recognize(&self, image_data: &[u8]) -> Result<Vec<UiElement>, CoreError>;
}
```

#### LlmProvider — LLM 추론
```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn resolve_intent(&self, intent: &AutomationIntent) -> Result<Vec<AutomationAction>, CoreError>;
}
```

## 에러 처리

```rust
#[derive(Debug, Error)]
pub enum CoreError {
    // 기본 에러
    #[error("직렬화 실패")] Serialization(#[from] serde_json::Error),
    #[error("설정 에러: {0}")] Config(String),
    #[error("유효성 검증 실패: {field}")] Validation { field: String, message: String },
    #[error("인증 실패: {0}")] Auth(String),
    #[error("{resource_type} 미발견: {id}")] NotFound { resource_type: String, id: String },
    #[error("내부 오류: {0}")] Internal(String),
    #[error("I/O 오류")] Io(#[from] std::io::Error),

    // 네트워크 에러
    #[error("네트워크 오류: {0}")] Network(String),
    #[error("요청 한도 초과")] RateLimit { retry_after_secs: Option<u64> },
    #[error("서비스 일시 불가: {0}")] ServiceUnavailable(String),

    // 정책/자동화 에러
    #[error("정책 거부: {0}")] PolicyDenied(String),
    #[error("잘못된 인자: {0}")] InvalidArguments(String),

    // 동의 에러
    #[error("동의 필요: {0}")] ConsentRequired(String),
    #[error("동의 만료")] ConsentExpired,

    // 샌드박스 에러
    #[error("샌드박스 초기화 실패: {0}")] SandboxInit(String),
    #[error("샌드박스 실행 실패: {0}")] SandboxExecution(String),
    #[error("샌드박스 미지원: {0}")] SandboxUnsupported(String),

    // 자동화 에러
    #[error("실행 타임아웃: {timeout_ms}ms")] ExecutionTimeout { timeout_ms: u64 },
    #[error("UI 요소 미발견: {0}")] ElementNotFound(String),
    #[error("프라이버시 거부: {0}")] PrivacyDenied(String),
    #[error("OCR 처리 실패: {0}")] OcrError(String),
}
```

## 설정 구조

```rust
pub struct AppConfig {
    pub server: ServerConfig,
    pub monitor: MonitorConfig,
    pub storage: StorageConfig,
    pub vision: VisionConfig,
    pub update: UpdateConfig,
    pub web: WebConfig,
    pub notification: NotificationConfig,
    pub telemetry: TelemetryConfig,
    pub privacy: PrivacyConfig,
    pub schedule: ScheduleConfig,
    pub file_access: FileAccessConfig,
    // 자동화 시스템
    pub automation: AutomationConfig,
    pub ai_provider: AiProviderConfig,
}
```

### 자동화 설정

```rust
/// 자동화 제어 설정
pub struct AutomationConfig {
    pub enabled: bool,
    pub sandbox: SandboxConfig,
    pub custom_presets: Vec<WorkflowPreset>,
}

/// OS 네이티브 샌드박스 설정
pub struct SandboxConfig {
    pub enabled: bool,
    pub profile: SandboxProfile,     // Permissive | Standard | Strict
    pub allowed_read_paths: Vec<String>,
    pub allowed_write_paths: Vec<String>,
    pub allow_network: bool,
    pub max_memory_bytes: u64,
    pub max_cpu_time_ms: u64,
}

pub enum SandboxProfile {
    Permissive,  // 최소 제한
    Standard,    // 표준 제한 (기본)
    Strict,      // 엄격한 제한
}
```

### AI 제공자 설정

```rust
/// AI OCR/LLM 제공자 설정
pub struct AiProviderConfig {
    pub ocr_provider: OcrProviderType,   // Local | Remote
    pub llm_provider: LlmProviderType,   // Local | Remote
    pub ocr_api: Option<ExternalApiEndpoint>,
    pub llm_api: Option<ExternalApiEndpoint>,
    pub external_data_policy: ExternalDataPolicy,
    pub fallback_to_local: bool,
}

/// 외부 API 엔드포인트
pub struct ExternalApiEndpoint {
    pub endpoint: String,
    pub api_key: String,        // config.json에 직접 저장
    pub model: Option<String>,
    pub timeout_secs: u64,      // 기본 30초
}

/// 외부 데이터 전송 정책
pub enum ExternalDataPolicy {
    PiiFilterStrict,    // PII 필터 Strict + 민감 앱 차단
    PiiFilterStandard,  // PII 필터 Standard
    AllowFiltered,      // 사용자 설정 그대로
}

/// AI API 제공자 타입
///
/// URL 문자열 매칭 대신 명시적 enum으로 제공자를 구분한다.
/// OSS 아키텍처에서 특정 벤더가 특권을 갖지 않도록 설정 파일 주도 방식으로 설계.
/// `ai_llm_client/`와 `ai_ocr_client/` (ADR-003 디렉토리 모듈)가 이 값을 읽어 요청 형식과
/// 인증 헤더를 결정하는 벤더 중립적 분기를 수행한다.
pub enum AiProviderType {
    Anthropic,  // Anthropic Claude API — x-api-key 헤더 + /v1/messages 형식
    OpenAi,     // OpenAI 호환 API — Authorization: Bearer 헤더 + /v1/chat/completions 형식
    Generic,    // 기타 제공자 — 커스텀 헤더 없음, 범용 응답 파싱 사용 (기본값)
}
```

## 의존성

이 크레이트는 최소 의존성을 유지합니다:
- `serde`: 직렬화
- `chrono`: 시간 처리
- `thiserror`: 에러 derive
- `async-trait`: 비동기 trait
- `tokio`: mpsc 채널

## 사용 예시

```rust
use oneshim_core::models::suggestion::Suggestion;
use oneshim_core::models::intent::AutomationIntent;
use oneshim_core::ports::api_client::ApiClient;
use oneshim_core::error::CoreError;

async fn handle_suggestion(
    client: &dyn ApiClient,
    suggestion: &Suggestion,
) -> Result<(), CoreError> {
    client.send_feedback(&suggestion.suggestion_id, true).await
}
```
