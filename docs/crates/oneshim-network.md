[English](./oneshim-network.md) | [한국어](./oneshim-network.ko.md)

# oneshim-network

The network adapter crate responsible for HTTP/SSE/WebSocket/gRPC communication.

## Role

- **Server Communication**: REST API requests, SSE stream reception, gRPC RPC calls
- **Authentication Management**: JWT token acquisition/refresh/storage
- **Data Compression**: Adaptive compression algorithm selection
- **Batch Upload**: Batch transmission of events/frames
- **gRPC Client**: Authentication, session, and context services (Feature Flag)

## Directory Structure

```
oneshim-network/src/
├── lib.rs                  # Crate root
├── auth.rs                 # TokenManager - JWT authentication
├── http_client.rs          # HttpApiClient - REST API
├── sse_client.rs           # SseStreamClient - SSE reception
├── compression.rs          # AdaptiveCompressor
├── batch_uploader.rs       # BatchUploader - batch transmission
├── circuit_breaker.rs      # per-endpoint circuit breaker (serial_test guarded)
├── connectivity.rs         # connectivity detection + backoff
├── resilience.rs           # shared resilience primitives
├── error.rs                # NetworkError enum (13 variants, typed-code per ADR-019)
├── ai_llm_client/          # RemoteLlmProvider — directory module (ADR-003)
│   ├── mod.rs              # RemoteLlmProvider struct + LlmProvider impl
│   ├── request.rs          # request building per provider type
│   ├── parsers.rs          # response parsing + extraction
│   └── tests.rs            # unit tests
├── ai_ocr_client/          # RemoteOcrProvider — directory module (ADR-003)
│   ├── mod.rs              # RemoteOcrProvider struct + OcrProvider impl
│   ├── ollama.rs           # Ollama-specific handling
│   ├── parsers.rs          # element extraction + JSON parsing
│   ├── strategy.rs         # provider strategy selection
│   └── tests.rs            # unit tests
├── http_api_session/       # HTTP-based stateful ApiSession (chat + tool-calling)
│   ├── mod.rs              # orchestrator
│   ├── anthropic.rs, google.rs, openai.rs  # provider-specific request/response builders
│   ├── content.rs          # content block types
│   └── tests.rs            # unit tests
├── local_llm_session.rs    # local ApiSession via subprocess LLM
├── analysis_client.rs      # analysis provider client
├── remote_embedding_client.rs # remote embedding provider
├── sync/                   # Cross-device sync
│   ├── lan_server/         # LAN peer discovery server (directory module)
│   └── lan_transport/      # LAN transport client (directory module)
├── integration/            # External integration transports
│   ├── http_transport/     # HTTP remote transport (directory module)
│   ├── auth/               # Integration auth (OIDC device flow, etc.)
│   ├── live_channel.rs     # WebSocket transport (active — replaced deleted ws_client.rs)
│   └── session_coordinator.rs # session transport kind switching
├── oauth/                  # OAuth 2.0 flow helpers (directory module)
├── grpc/                   # gRPC client (#[cfg(feature = "grpc")])
│   ├── mod.rs              # module exports
│   ├── config.rs           # GrpcConfig (endpoints, fallback ports, TLS)
│   ├── auth_client.rs      # GrpcAuthClient
│   ├── session_client.rs   # GrpcSessionClient
│   ├── context_client.rs   # GrpcContextClient
│   ├── health_client.rs    # GrpcHealthClient (Consumer Contract ClientHealth.Ping)
│   ├── unified_client.rs   # UnifiedClient (gRPC + REST unified with auto-switching)
│   ├── api_adapter.rs      # GrpcApiAdapter (impl ApiClient bridging + REST fallback)
│   ├── sse_adapter.rs      # GrpcSseAdapter (impl SseClient bridging gRPC streaming)
│   └── error_mapping.rs    # tonic Status → NetworkError conversion
└── proto/                  # tonic/prost generated code
    └── oneshim/v1/, oneshim/client/v1/
```

## Key Components

### TokenManager (auth.rs)

JWT token lifecycle management:

```rust
pub struct TokenManager {
    base_url: String,
    http_client: reqwest::Client,
    token_state: RwLock<TokenState>,
}

impl TokenManager {
    /// Get token (returns cached token if available, otherwise logs in)
    pub async fn get_token(&self) -> Result<String, CoreError>;

    /// Refresh token
    pub async fn refresh_token(&self) -> Result<String, CoreError>;

    /// Logout
    pub async fn logout(&self) -> Result<(), CoreError>;
}
```

**Features**:
- Email/password login
- Automatic refresh before token expiration
- Thread-safe token management via `RwLock`

### HttpApiClient (http_client.rs)

REST API communication implementation (`ApiClient` port):

```rust
pub struct HttpApiClient {
    base_url: String,
    http_client: reqwest::Client,
    token_manager: Arc<TokenManager>,
    compressor: Arc<dyn Compressor>,
}
```

**Endpoints**:
| Method | Path | Description |
|--------|------|-------------|
| POST | `/user_context/contexts` | Upload context |
| POST | `/user_context/frames` | Batch upload frames |
| POST | `/suggestions/{id}/feedback` | Suggestion feedback |

### SseStreamClient (sse_client.rs)

Server-Sent Events reception (`SseClient` port):

```rust
pub struct SseStreamClient {
    base_url: String,
    token_manager: Arc<TokenManager>,
    max_retry_secs: u64,
    http_client: reqwest::Client,
}
```

**Features**:
- SSE connection based on `reqwest-eventsource`
- Exponential backoff reconnection (1s → 30s)
- Per-event-type parsing: `connection`, `suggestion`, `heartbeat`, `update`, `error`, `close`

**SSE Event Flow**:
```
Server → SseStreamClient → mpsc::Sender<SseEvent> → SuggestionReceiver
```

### AdaptiveCompressor (compression.rs)

Adaptive compression algorithm selection (`Compressor` port):

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

**Algorithm Selection Criteria**:
| Data Size | Algorithm | Reason |
|-----------|-----------|--------|
| ≤ 1KB | None | Overhead exceeds compression benefit |
| 1KB-10KB | LZ4 | Fast compression/decompression |
| 10KB-100KB | Gzip | Balanced compression ratio |
| > 100KB | Zstd | Best compression ratio |

### BatchUploader (batch_uploader.rs)

Batch transmission of events/frames:

```rust
pub struct BatchUploader {
    api_client: Arc<dyn ApiClient>,
    /// Lock-free queue — concurrent push from multiple producers (crossbeam SegQueue)
    queue: Arc<SegQueue<Event>>,
    /// Queue size (lock-free counter)
    queue_size: AtomicUsize,
    session_id: String,
    max_batch_size: usize,
    max_retries: u32,
    /// Dynamic batch size enabled
    dynamic_batch: bool,
}
```

**Features**:
- Lock-free MPSC queue based on `crossbeam::SegQueue` — contention-free enqueue from multiple threads
- Lock-free queue size tracking via `AtomicUsize`
- Dynamic batch size: immediate send when queue < 10, double batch for fast processing when > 50
- Automatic event re-queuing on failure, exponential backoff retry (max 3 attempts)

### AI Clients

Two clients that call external AI APIs. Both branch by provider using the `AiProviderType` enum from `oneshim-core`.
Versioned contract reference: `docs/contracts/ai-provider-contract.md`.

```rust
// oneshim-core/config.rs
pub enum AiProviderType {
    Anthropic,  // Claude API
    OpenAi,     // OpenAI-compatible API
    Google,     // Google Gemini/Vision API
    Generic,    // Generic JSON endpoint
}
```

#### RemoteLlmProvider (ai_llm_client/)

`LlmProvider` port implementation — sends sanitized text to an external LLM API to interpret UI automation intents.
Does not send images; only PII-filtered text is transmitted through the Privacy Gateway.

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
    /// Receives screen context and intent hint, returns InterpretedAction
    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError>;
}
```

**Provider-specific branching**:
- `AiProviderType::Anthropic` → `POST /v1/messages`, `x-api-key` header, parses `content[0].text`
- `AiProviderType::OpenAi` / `Generic` → `POST /v1/chat/completions`, `Bearer` token, parses `choices[0].message.content`
- `AiProviderType::Google` → parses `candidates[0].content.parts[0].text`

#### RemoteOcrProvider (ai_ocr_client/)

`OcrProvider` port implementation — sends screenshots (Base64) to an external AI Vision API to extract text and bounding boxes.
PII blurring through the Privacy Gateway and user consent verification are required before sending images.

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
    /// Receives Base64-encoded image and returns OcrResult list
    async fn extract_elements(
        &self,
        image: &[u8],
        image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError>;
}
```

**Provider-specific branching**:
- `AiProviderType::Anthropic` → Claude Vision format (line-by-line parsing of `content[].text`)
- `AiProviderType::Google` → Google Vision `textAnnotations` parsing with bounding poly conversion
- `AiProviderType::OpenAi` → OpenAI content parsing with JSON-object preference (`{ "results": [...] }`) and line-text fallback
- `AiProviderType::Generic` → `{ "results": [...] }` parsing with provider-format fallback

## gRPC Client (`#[cfg(feature = "grpc")]`)

### GrpcConfig

```rust
pub struct GrpcConfig {
    pub use_grpc_auth: bool,      // Whether to use gRPC authentication
    pub use_grpc_context: bool,   // Whether to use gRPC context
    pub grpc_endpoint: String,    // gRPC server address
    pub rest_endpoint: String,    // REST fallback address
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub use_tls: bool,
}
```

### GrpcAuthClient (grpc/auth_client.rs)

Authentication service gRPC client:

| RPC | Description |
|-----|-------------|
| `Login` | Login (identifier, password, organization_id) |
| `Logout` | Logout |
| `RefreshToken` | Token refresh |
| `ValidateToken` | Token validation |

### GrpcSessionClient (grpc/session_client.rs)

Session service gRPC client:

| RPC | Description |
|-----|-------------|
| `CreateSession` | Create session (client_id, device_info) |
| `EndSession` | End session |
| `Heartbeat` | Session heartbeat |
| `GetSession` | Get session |

### GrpcContextClient (grpc/context_client.rs)

User context service gRPC client:

| RPC | Description |
|-----|-------------|
| `UploadBatch` | Batch upload of events/frames |
| `SubscribeSuggestions` | Subscribe to suggestion stream (Server Streaming) |
| `SendFeedback` | Send suggestion feedback |
| `ListSuggestions` | List suggestions (type filtering) |
| `Heartbeat` | Context heartbeat |

### UnifiedClient (grpc/unified_client.rs)

A hybrid client that unifies gRPC and REST:

```rust
pub struct UnifiedClient {
    config: GrpcConfig,
    token_manager: Arc<TokenManager>,
    http_client: HttpApiClient,
    // gRPC clients are lazily initialized
}

impl UnifiedClient {
    /// gRPC first, REST fallback on failure
    pub async fn upload_batch(&self, request: ContextBatchUploadRequest)
        -> Result<ContextBatchUploadResponse, CoreError>;

    /// Server Streaming RPC
    pub async fn subscribe_suggestions(&self, session_id: &str, client_id: &str)
        -> Result<tonic::Streaming<Suggestion>, CoreError>;
}
```

**Feature Flag-based switching**:
- `use_grpc_auth: true` → gRPC authentication, REST on failure
- `use_grpc_context: true` → gRPC context, REST on failure
- Industrial environment support: automatic REST fallback in HTTP/2 blocked environments

## Authentication Flow

```
┌─────────────┐     ┌──────────────┐     ┌─────────┐
│ HttpClient  │────▶│ TokenManager │────▶│ Server  │
└─────────────┘     └──────────────┘     └─────────┘
       │                   │
       │  get_token()      │
       │──────────────────▶│
       │                   │  (check cache)
       │                   │  (refresh if expired)
       │◀──────────────────│
       │   Bearer Token    │
```

## Dependencies

- `reqwest`: HTTP client (native-tls backend)
- `reqwest-eventsource`: SSE stream
- `tokio-tungstenite`: WebSocket (native-tls)
- `flate2`: Gzip compression
- `zstd`: Zstd compression
- `lz4_flex`: LZ4 compression
- `tonic`: gRPC client (`#[cfg(feature = "grpc")]`)
- `prost`: Protocol Buffers messages (`#[cfg(feature = "grpc")]`)

## Tests

```rust
#[tokio::test]
async fn test_compression_selection() {
    let compressor = AdaptiveCompressor::default();

    // Small data: no compression
    let small = vec![0u8; 500];
    let (_, comp_type) = compressor.compress(&small).unwrap();
    assert_eq!(comp_type, CompressionType::None);

    // Large data: Zstd
    let large = vec![0u8; 200_000];
    let (_, comp_type) = compressor.compress(&large).unwrap();
    assert_eq!(comp_type, CompressionType::Zstd);
}
```

## Usage Example

```rust
use oneshim_network::{TokenManager, HttpApiClient, SseStreamClient};

// Initialization
let token_manager = Arc::new(TokenManager::new("https://api.example.com", "user@email.com", "password"));
let api_client = HttpApiClient::new("https://api.example.com", token_manager.clone(), compressor);
let sse_client = SseStreamClient::new("https://api.example.com", token_manager, 30);

// SSE connection
let (tx, mut rx) = mpsc::channel(100);
tokio::spawn(async move {
    sse_client.connect("session_123", tx).await
});

// Event reception
while let Some(event) = rx.recv().await {
    match event {
        SseEvent::Suggestion(s) => handle_suggestion(s),
        SseEvent::Heartbeat { timestamp } => log_heartbeat(timestamp),
        _ => {}
    }
}
```
