[English](./oneshim-network.md) | [한국어](./oneshim-network.ko.md)

# oneshim-network

HTTP/SSE/WebSocket/gRPC 통신을 담당하는 네트워크 어댑터 크레이트.

## 역할

- **서버 통신**: REST API 요청, SSE 스트림 수신, gRPC RPC 호출
- **인증 관리**: JWT 토큰 획득/갱신/저장
- **데이터 압축**: 적응형 압축 알고리즘 선택
- **배치 업로드**: 이벤트/프레임 배치 전송
- **gRPC 클라이언트**: 인증, 세션, 컨텍스트 서비스 (Feature Flag)

## 디렉토리 구조

```
oneshim-network/src/
├── lib.rs                  # 크레이트 루트
├── auth.rs                 # TokenManager - JWT 인증
├── http_client.rs          # HttpApiClient - REST API
├── sse_client.rs           # SseStreamClient - SSE 수신
├── compression.rs          # AdaptiveCompressor
├── batch_uploader.rs       # BatchUploader - 배치 전송
├── circuit_breaker.rs      # 엔드포인트별 circuit breaker (serial_test 가드)
├── connectivity.rs         # 연결성 감지 + backoff
├── resilience.rs           # 공용 resilience 프리미티브
├── error.rs                # NetworkError enum (13개 변형, ADR-019 typed-code)
├── ai_llm_client/          # RemoteLlmProvider — 디렉토리 모듈 (ADR-003)
│   ├── mod.rs, request.rs, parsers.rs, tests.rs
├── ai_ocr_client/          # RemoteOcrProvider — 디렉토리 모듈 (ADR-003)
│   ├── mod.rs, ollama.rs, parsers.rs, strategy.rs, tests.rs
├── http_api_session/       # HTTP 기반 stateful ApiSession (chat + tool-calling)
│   ├── mod.rs, anthropic.rs, google.rs, openai.rs, content.rs, tests.rs
├── local_llm_session.rs    # subprocess LLM 기반 local ApiSession
├── analysis_client.rs      # analysis provider 클라이언트
├── remote_embedding_client.rs  # remote embedding provider
├── sync/                   # Cross-device sync
│   ├── lan_server/         # LAN peer discovery 서버 (디렉토리 모듈)
│   └── lan_transport/      # LAN transport 클라이언트 (디렉토리 모듈)
├── integration/            # 외부 integration transports
│   ├── http_transport/, auth/, live_channel.rs (WebSocket — 삭제된 ws_client.rs 대체)
│   └── session_coordinator.rs
├── oauth/                  # OAuth 2.0 flow 헬퍼 (디렉토리 모듈)
├── grpc/                   # gRPC 클라이언트 (#[cfg(feature = "grpc")])
│   ├── mod.rs, config.rs, auth_client.rs, session_client.rs
│   ├── context_client.rs, health_client.rs
│   ├── unified_client.rs   # gRPC + REST 자동 스위칭
│   ├── api_adapter.rs, sse_adapter.rs  # ApiClient / SseClient 브리지
│   └── error_mapping.rs    # tonic Status → NetworkError 변환
└── proto/                  # tonic/prost 생성 코드
    └── oneshim/v1/, oneshim/client/v1/
```

## 주요 컴포넌트

### TokenManager (auth.rs)

JWT 토큰 라이프사이클 관리:

```rust
pub struct TokenManager {
    base_url: String,
    http_client: reqwest::Client,
    token_state: RwLock<TokenState>,
}

impl TokenManager {
    /// 토큰 획득 (캐시된 토큰이 있으면 반환, 없으면 로그인)
    pub async fn get_token(&self) -> Result<String, CoreError>;

    /// 토큰 갱신
    pub async fn refresh_token(&self) -> Result<String, CoreError>;

    /// 로그아웃
    pub async fn logout(&self) -> Result<(), CoreError>;
}
```

**기능**:
- 이메일/비밀번호 로그인
- 토큰 만료 전 자동 갱신
- `RwLock`으로 thread-safe 토큰 관리

### HttpApiClient (http_client.rs)

REST API 통신 구현 (`ApiClient` 포트):

```rust
pub struct HttpApiClient {
    base_url: String,
    http_client: reqwest::Client,
    token_manager: Arc<TokenManager>,
    compressor: Arc<dyn Compressor>,
}
```

**엔드포인트**:
| 메서드 | 경로 | 설명 |
|--------|------|------|
| POST | `/user_context/contexts` | 컨텍스트 업로드 |
| POST | `/user_context/frames` | 프레임 배치 업로드 |
| POST | `/suggestions/{id}/feedback` | 제안 피드백 |

### SseStreamClient (sse_client.rs)

Server-Sent Events 수신 (`SseClient` 포트):

```rust
pub struct SseStreamClient {
    base_url: String,
    token_manager: Arc<TokenManager>,
    max_retry_secs: u64,
    http_client: reqwest::Client,
}
```

**기능**:
- `reqwest-eventsource` 기반 SSE 연결
- Exponential backoff 재연결 (1s → 30s)
- 이벤트 타입별 파싱: `connection`, `suggestion`, `heartbeat`, `update`, `error`, `close`

**SSE 이벤트 흐름**:
```
서버 → SseStreamClient → mpsc::Sender<SseEvent> → SuggestionReceiver
```

### AdaptiveCompressor (compression.rs)

적응형 압축 알고리즘 선택 (`Compressor` 포트):

```rust
pub struct AdaptiveCompressor {
    gzip_threshold: usize,
    zstd_threshold: usize,
}

impl AdaptiveCompressor {
    pub fn compress(&self, data: &[u8]) -> Result<(Vec<u8>, CompressionType), CoreError> {
        match data.len() {
            0..=1024 => Ok((data.to_vec(), CompressionType::None)),
            1025..=10240 => self.compress_lz4(data),
            10241..=102400 => self.compress_gzip(data),
            _ => self.compress_zstd(data),
        }
    }
}
```

**알고리즘 선택 기준**:
| 데이터 크기 | 알고리즘 | 이유 |
|------------|----------|------|
| ≤ 1KB | None | 오버헤드가 압축 이득보다 큼 |
| 1KB-10KB | LZ4 | 빠른 압축/해제 |
| 10KB-100KB | Gzip | 균형 잡힌 압축률 |
| > 100KB | Zstd | 최고 압축률 |

### BatchUploader (batch_uploader.rs)

이벤트/프레임 배치 전송:

```rust
pub struct BatchUploader {
    api_client: Arc<dyn ApiClient>,
    /// Lock-free 큐 — 여러 producer에서 동시 push 가능 (crossbeam SegQueue)
    queue: Arc<SegQueue<Event>>,
    /// 큐 크기 (lock-free 카운터)
    queue_size: AtomicUsize,
    session_id: String,
    max_batch_size: usize,
    max_retries: u32,
    /// 동적 배치 크기 활성화
    dynamic_batch: bool,
}
```

**기능**:
- `crossbeam::SegQueue` 기반 lock-free MPSC 큐 — 여러 스레드에서 무경합 enqueue
- `AtomicUsize`로 락 없이 큐 크기 추적
- 동적 배치 크기: 큐 10개 미만이면 즉시 전송, 50개 초과이면 2배 배치로 빠른 처리
- 실패 시 이벤트 자동 재큐잉, exponential backoff 재시도 (최대 3회)

### AI 클라이언트

외부 AI API를 호출하는 두 클라이언트. 모두 `oneshim-core`의 `AiProviderType` enum으로 제공자를 분기한다.
버전드 계약 문서 참조: `docs/contracts/ai-provider-contract.ko.md`.

```rust
// oneshim-core/config.rs
pub enum AiProviderType {
    Anthropic,  // Claude API
    OpenAi,     // OpenAI 호환 API
    Google,     // Google Gemini/Vision API
    Generic,    // 범용 JSON 엔드포인트
}
```

#### RemoteLlmProvider (ai_llm_client/)

`LlmProvider` 포트 구현 — 세정된 텍스트를 외부 LLM API로 전송해 UI 자동화 의도를 해석한다.
이미지는 전송하지 않으며, Privacy Gateway를 통해 PII 필터가 적용된 텍스트만 전달한다.

```rust
pub struct RemoteLlmProvider {
    http_client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    provider_type: AiProviderType,
    timeout_secs: u64,
}

impl LlmProvider for RemoteLlmProvider {
    /// 화면 컨텍스트와 의도 힌트를 받아 InterpretedAction 반환
    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError>;
}
```

**제공자별 분기**:
- `AiProviderType::Anthropic` → `POST /v1/messages`, `x-api-key` 헤더, `content[0].text` 파싱
- `AiProviderType::OpenAi` / `Generic` → `POST /v1/chat/completions`, `Bearer` 토큰, `choices[0].message.content` 파싱
- `AiProviderType::Google` → `candidates[0].content.parts[0].text` 파싱

#### RemoteOcrProvider (ai_ocr_client/)

`OcrProvider` 포트 구현 — 스크린샷(Base64)을 외부 AI Vision API로 전송해 텍스트와 바운딩 박스를 추출한다.
이미지 전송 전 Privacy Gateway를 통한 PII 블러 및 사용자 동의 확인이 필수다.

```rust
pub struct RemoteOcrProvider {
    http_client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: Option<String>,
    provider_type: AiProviderType,
    timeout_secs: u64,
}

impl OcrProvider for RemoteOcrProvider {
    /// Base64 인코딩 이미지를 받아 OcrResult 목록 반환
    async fn extract_elements(
        &self,
        image: &[u8],
        image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError>;
}
```

**제공자별 분기**:
- `AiProviderType::Anthropic` → Claude Vision 형식 (`content[].text` 줄 단위 파싱)
- `AiProviderType::Google` → Google Vision `textAnnotations` + bounding poly 변환
- `AiProviderType::OpenAi` → OpenAI content 파싱 후 JSON 객체(`{ "results": [...] }`) 우선, 줄 텍스트 파싱 폴백
- `AiProviderType::Generic` → `{ "results": [...] }` 우선 파싱 + 제공자 형식 폴백

## gRPC 클라이언트 (`#[cfg(feature = "grpc")]`)

### GrpcConfig

```rust
pub struct GrpcConfig {
    pub use_grpc_auth: bool,      // gRPC 인증 사용 여부
    pub use_grpc_context: bool,   // gRPC 컨텍스트 사용 여부
    pub grpc_endpoint: String,    // gRPC 서버 주소
    pub rest_endpoint: String,    // REST fallback 주소
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub use_tls: bool,
}
```

### GrpcAuthClient (grpc/auth_client.rs)

인증 서비스 gRPC 클라이언트:

| RPC | 설명 |
|-----|------|
| `Login` | 로그인 (identifier, password, organization_id) |
| `Logout` | 로그아웃 |
| `RefreshToken` | 토큰 갱신 |
| `ValidateToken` | 토큰 검증 |

### GrpcSessionClient (grpc/session_client.rs)

세션 서비스 gRPC 클라이언트:

| RPC | 설명 |
|-----|------|
| `CreateSession` | 세션 생성 (client_id, device_info) |
| `EndSession` | 세션 종료 |
| `Heartbeat` | 세션 하트비트 |
| `GetSession` | 세션 조회 |

### GrpcContextClient (grpc/context_client.rs)

사용자 컨텍스트 서비스 gRPC 클라이언트:

| RPC | 설명 |
|-----|------|
| `UploadBatch` | 이벤트/프레임 배치 업로드 |
| `SubscribeSuggestions` | 제안 스트림 구독 (Server Streaming) |
| `SendFeedback` | 제안 피드백 전송 |
| `ListSuggestions` | 제안 목록 조회 (타입 필터링) |
| `Heartbeat` | 컨텍스트 하트비트 |

### UnifiedClient (grpc/unified_client.rs)

gRPC와 REST를 통합하는 하이브리드 클라이언트:

```rust
pub struct UnifiedClient {
    config: GrpcConfig,
    token_manager: Arc<TokenManager>,
    http_client: HttpApiClient,
    // gRPC 클라이언트는 지연 초기화
}

impl UnifiedClient {
    /// gRPC 우선, 실패 시 REST fallback
    pub async fn upload_batch(&self, request: ContextBatchUploadRequest)
        -> Result<ContextBatchUploadResponse, CoreError>;

    /// Server Streaming RPC
    pub async fn subscribe_suggestions(&self, session_id: &str, client_id: &str)
        -> Result<tonic::Streaming<Suggestion>, CoreError>;
}
```

**Feature Flag 기반 전환**:
- `use_grpc_auth: true` → gRPC 인증, 실패 시 REST
- `use_grpc_context: true` → gRPC 컨텍스트, 실패 시 REST
- 산업 현장 지원: HTTP/2 차단 환경에서 자동 REST fallback

## 인증 흐름

```
┌─────────────┐     ┌──────────────┐     ┌─────────┐
│ HttpClient  │────▶│ TokenManager │────▶│ Server  │
└─────────────┘     └──────────────┘     └─────────┘
       │                   │
       │  get_token()      │
       │──────────────────▶│
       │                   │  (캐시 확인)
       │                   │  (만료 시 갱신)
       │◀──────────────────│
       │   Bearer Token    │
```

## 의존성

- `reqwest`: HTTP 클라이언트 (native-tls 백엔드)
- `reqwest-eventsource`: SSE 스트림
- `tokio-tungstenite`: WebSocket (native-tls)
- `flate2`: Gzip 압축
- `zstd`: Zstd 압축
- `lz4_flex`: LZ4 압축
- `tonic`: gRPC 클라이언트 (`#[cfg(feature = "grpc")]`)
- `prost`: Protocol Buffers 메시지 (`#[cfg(feature = "grpc")]`)

## 테스트

```rust
#[tokio::test]
async fn test_compression_selection() {
    let compressor = AdaptiveCompressor::default();

    // 작은 데이터: 압축 안함
    let small = vec![0u8; 500];
    let (_, comp_type) = compressor.compress(&small).unwrap();
    assert_eq!(comp_type, CompressionType::None);

    // 큰 데이터: Zstd
    let large = vec![0u8; 200_000];
    let (_, comp_type) = compressor.compress(&large).unwrap();
    assert_eq!(comp_type, CompressionType::Zstd);
}
```

## 사용 예시

```rust
use oneshim_network::{TokenManager, HttpApiClient, SseStreamClient};

// 초기화
let token_manager = Arc::new(TokenManager::new("https://api.example.com", "user@email.com", "password"));
let api_client = HttpApiClient::new("https://api.example.com", token_manager.clone(), compressor);
let sse_client = SseStreamClient::new("https://api.example.com", token_manager, 30);

// SSE 연결
let (tx, mut rx) = mpsc::channel(100);
tokio::spawn(async move {
    sse_client.connect("session_123", tx).await
});

// 이벤트 수신
while let Some(event) = rx.recv().await {
    match event {
        SseEvent::Suggestion(s) => handle_suggestion(s),
        SseEvent::Heartbeat { timestamp } => log_heartbeat(timestamp),
        _ => {}
    }
}
```
