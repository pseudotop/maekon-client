[English](./oneshim-core.md) | [한국어](./oneshim-core.ko.md)

# oneshim-core

The core crate that defines domain models, port interfaces, error types, and configuration.

## Role

- **Central Hub**: The foundation layer that all other crates depend on
- **Contract Definition**: Standardizes adapter interfaces via Port traits
- **Type Safety**: Ensures consistent data flow through domain models and error types

## Directory Structure

```
oneshim-core/src/
├── lib.rs           # Crate root, module re-exports
├── config/          # AppConfig and configuration sections — directory module (ADR-003)
│   ├── mod.rs       # AppConfig struct + Default + re-exports
│   ├── enums.rs     # PiiFilterLevel, Weekday, SandboxProfile, AiAccessMode, etc.
│   └── sections/    # 37 config section structs across per-domain files + Default impls (directory module per ADR-003)
├── config_manager.rs # JSON-based config file management + platform-specific paths
├── consent.rs       # ConsentManager, GDPR Article 17/20 compliance
├── error.rs         # CoreError enum (thiserror, 38 variants; typed `code: XxxCode` fields per ADR-019)
├── error_codes/     # 18 typed code enums generated via `define_code_enum!` macro (ADR-019)
├── models/          # Domain models
│   ├── mod.rs
│   ├── suggestion.rs   # Suggestion, SuggestionType, Priority
│   ├── event.rs        # ContextEvent, EventType, InputActivityEvent
│   ├── frame.rs        # CapturedFrame, FrameMetadata, ProcessedFrame
│   ├── context.rs      # ContextPayload, DeviceInfo
│   ├── session.rs      # SessionInfo, SessionStatus
│   ├── system.rs       # SystemMetrics, CpuMetrics, MemoryMetrics
│   ├── telemetry.rs    # Telemetry-related models
│   ├── automation.rs   # AutomationAction, MouseButton
│   └── intent.rs       # AutomationIntent, UiElement, WorkflowPreset
└── ports/           # Port interfaces (traits)
    ├── mod.rs
    ├── api_client.rs   # ApiClient, SseClient, SseEvent
    ├── storage.rs      # StorageService
    ├── monitor.rs      # SystemMonitor, ProcessMonitor, ActivityMonitor
    ├── vision.rs       # CaptureTrigger, FrameProcessor
    ├── notifier.rs     # DesktopNotifier
    ├── compressor.rs   # Compressor
    ├── element_finder.rs # ElementFinder — UI element discovery
    ├── input_driver.rs   # InputDriver — mouse/keyboard input
    ├── ocr_provider.rs   # OcrProvider — OCR text recognition
    ├── llm_provider.rs   # LlmProvider — LLM inference
    └── sandbox.rs        # Sandbox — OS native sandbox
```

## Key Models

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

### Automation Models

#### AutomationAction — Low-Level Automation Action
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

#### AutomationIntent — Server→Client High-Level Intent
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

#### UiElement — UI Element Found on Screen
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

#### WorkflowPreset — Workflow Preset
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
    Productivity,    // Productivity
    AppManagement,   // App management
    Workflow,        // Workflow
    Custom,          // User-defined
}
```

#### IntentResult / IntentConfig — Execution Result and Configuration
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

## Port Interfaces

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

### Automation Ports

#### Sandbox — OS Native Kernel Sandbox
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

#### ElementFinder — UI Element Discovery
```rust
#[async_trait]
pub trait ElementFinder: Send + Sync {
    async fn find_element(&self, text: &str, role: Option<&str>) -> Result<Vec<UiElement>, CoreError>;
}
```

#### InputDriver — Mouse/Keyboard Input
```rust
#[async_trait]
pub trait InputDriver: Send + Sync {
    async fn execute_action(&self, action: &AutomationAction) -> Result<(), CoreError>;
}
```

#### OcrProvider — OCR Text Recognition
```rust
#[async_trait]
pub trait OcrProvider: Send + Sync {
    async fn recognize(&self, image_data: &[u8]) -> Result<Vec<UiElement>, CoreError>;
}
```

#### LlmProvider — LLM Inference
```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn resolve_intent(&self, intent: &AutomationIntent) -> Result<Vec<AutomationAction>, CoreError>;
}
```

## Error Handling

```rust
#[derive(Debug, Error)]
pub enum CoreError {
    // Basic errors
    #[error("Serialization failed")] Serialization(#[from] serde_json::Error),
    #[error("Config error: {0}")] Config(String),
    #[error("Validation failed: {field}")] Validation { field: String, message: String },
    #[error("Authentication failed: {0}")] Auth(String),
    #[error("{resource_type} not found: {id}")] NotFound { resource_type: String, id: String },
    #[error("Internal error: {0}")] Internal(String),
    #[error("I/O error")] Io(#[from] std::io::Error),

    // Network errors
    #[error("Network error: {0}")] Network(String),
    #[error("Rate limit exceeded")] RateLimit { retry_after_secs: Option<u64> },
    #[error("Service temporarily unavailable: {0}")] ServiceUnavailable(String),

    // Policy/automation errors
    #[error("Policy denied: {0}")] PolicyDenied(String),
    #[error("Invalid arguments: {0}")] InvalidArguments(String),

    // Consent errors
    #[error("Consent required: {0}")] ConsentRequired(String),
    #[error("Consent expired")] ConsentExpired,

    // Sandbox errors
    #[error("Sandbox init failed: {0}")] SandboxInit(String),
    #[error("Sandbox execution failed: {0}")] SandboxExecution(String),
    #[error("Sandbox unsupported: {0}")] SandboxUnsupported(String),

    // Automation errors
    #[error("Execution timeout: {timeout_ms}ms")] ExecutionTimeout { timeout_ms: u64 },
    #[error("UI element not found: {0}")] ElementNotFound(String),
    #[error("Privacy denied: {0}")] PrivacyDenied(String),
    #[error("OCR processing failed: {0}")] OcrError(String),
}
```

## Configuration Structure

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
    // Automation system
    pub automation: AutomationConfig,
    pub ai_provider: AiProviderConfig,
}
```

### Automation Configuration

```rust
/// Automation control configuration
pub struct AutomationConfig {
    pub enabled: bool,
    pub sandbox: SandboxConfig,
    pub custom_presets: Vec<WorkflowPreset>,
}

/// OS native sandbox configuration
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
    Permissive,  // Minimal restrictions
    Standard,    // Standard restrictions (default)
    Strict,      // Strict restrictions
}
```

### AI Provider Configuration

```rust
/// AI OCR/LLM provider configuration
pub struct AiProviderConfig {
    pub ocr_provider: OcrProviderType,   // Local | Remote
    pub llm_provider: LlmProviderType,   // Local | Remote
    pub ocr_api: Option<ExternalApiEndpoint>,
    pub llm_api: Option<ExternalApiEndpoint>,
    pub external_data_policy: ExternalDataPolicy,
    pub fallback_to_local: bool,
}

/// External API endpoint
pub struct ExternalApiEndpoint {
    pub endpoint: String,
    pub api_key: String,        // Stored directly in config.json
    pub model: Option<String>,
    pub timeout_secs: u64,      // Default 30 seconds
}

/// External data transmission policy
pub enum ExternalDataPolicy {
    PiiFilterStrict,    // PII filter Strict + sensitive app blocking
    PiiFilterStandard,  // PII filter Standard
    AllowFiltered,      // Use user settings as-is
}

/// AI API provider type
///
/// Distinguishes providers via explicit enum instead of URL string matching.
/// Designed with a config-file-driven approach so no specific vendor holds
/// privileges in the OSS architecture.
/// `ai_llm_client/` and `ai_ocr_client/` (directory modules per ADR-003) read this value to perform
/// vendor-neutral branching that determines request format and auth headers.
pub enum AiProviderType {
    Anthropic,  // Anthropic Claude API — x-api-key header + /v1/messages format
    OpenAi,     // OpenAI-compatible API — Authorization: Bearer header + /v1/chat/completions format
    Generic,    // Other providers — no custom headers, uses generic response parsing (default)
}
```

## Dependencies

This crate maintains minimal dependencies:
- `serde`: Serialization
- `chrono`: Time handling
- `thiserror`: Error derive
- `async-trait`: Async traits
- `tokio`: mpsc channels

## Usage Example

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
