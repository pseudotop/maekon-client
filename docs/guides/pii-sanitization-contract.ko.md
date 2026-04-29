# PII 살균(sanitization) 계약

**Status**: Accepted 2026-04-20 (D5 PII Filter Audit)
**범위**: `client-rust` 워크스페이스의 모든 텍스트 생산 어댑터 + 하위 write/send 사이트
**관련**: D5 설계 스펙과 감사 매트릭스는 내부 implementation record 로 보관합니다.

## 규칙

**저장 또는 전송 경계를 넘는 모든 텍스트 값은 반드시 살균(sanitize)해야 한다.**

여기서 "경계"는 데이터가 실행 중인 에이전트 프로세스의 인메모리 신뢰 도메인을 벗어나는 모든 지점을 가리킨다:

| 경계 | 예시 |
|------|------|
| SQLite write | `frames.ocr_text`, `local_suggestions.content`, `ai_sessions.state`, `coaching_events.personalized_message` |
| 서버 업로드 | `BatchUploader::enqueue` 이벤트, 피드백 제출, 텔레메트리 리포트 |
| 외부 API 요청 바디 | LLM provider chat, OCR provider vision, embedding provider, audio STT cloud |
| 크로스-디바이스 sync egress | `SyncExtractor` 직렬화된 payload (exemption 참조) |
| 감사 로그 entry | `AuditLogger::record` 명령 출력 |
| 구조화 `tracing` 필드 값 | 로깅되는 `user_input` / `message` 단편 (설계상 PII-free인 `err.code`는 제외) |
| 데스크탑 알림 바디 | `DesktopNotifier::show_notification` 으로 렌더링되는 title + body |
| Export 파일 | `/api/export/*` 핸들러에서 생성되는 CSV / JSON / iCal |

## 살균 레벨 결정

1차 출처: `config.privacy.pii_filter_level` — 사용자 설정 가능한 4단 계단식:

| 레벨 | 마스킹 대상 |
|------|-------------|
| `Off` | 없음 (살균 우회; 감사 로그 가능한 선택) |
| `Basic` | 이메일, 전화번호 |
| `Standard` | Basic 포함 + 신용카드, 주민번호, SSN, IBAN, 사용자 경로 |
| `Strict` | Standard 포함 + API 키, IP 주소, 여권번호 |

외부 경로 경계는 레벨을 상향 조정할 수 있으나 (예: `ExternalDataPolicy::PiiFilterStrict`는 외부 AI 제공자로 전송 시 Strict 강제), 사용자 설정 레벨 아래로 내릴 수 없다.

## 적용 방법

### `src-tauri/` 바이너리 crate 내부

```rust
use oneshim_vision::privacy::sanitize_title_with_level;
use oneshim_core::config::PiiFilterLevel;

// 경계에서:
let sanitized = sanitize_title_with_level(&raw_text, pii_filter_level);
storage.save(&sanitized)?;
```

### 어댑터 crate 내부 (`oneshim-network`, `oneshim-audio`, `oneshim-automation`, `oneshim-analysis`, `oneshim-monitor`)

repository hexagonal architecture guardrail 에 따라 (금지: 어댑터 crate 간 직접 의존성), port trait 경유 주입 필수:

```rust
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use std::sync::Arc;

pub struct MyAdapter {
    // ... 기존 필드 ...
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_filter_level: PiiFilterLevel,
}

impl MyAdapter {
    pub fn with_pii_sanitizer(mut self, s: Arc<dyn PiiSanitizer>) -> Self {
        self.pii_sanitizer = Some(s);
        self
    }

    fn sanitize(&self, text: &str) -> String {
        self.pii_sanitizer
            .as_ref()
            .map(|s| s.sanitize_text(text, self.pii_filter_level))
            .unwrap_or_else(|| text.to_string())
    }
}
```

DI 시점 (`src-tauri/src/main.rs` 또는 `agent_runtime_support.rs`):

```rust
let sanitizer: Arc<dyn PiiSanitizer> =
    Arc::new(oneshim_vision::privacy::VisionPiiSanitizer);
let adapter = MyAdapter::new(...).with_pii_sanitizer(sanitizer.clone());
```

## 예외(Exemptions)

다음 조건에서 경로가 살균에서 제외될 수 있음:

1. **프로세스 내부 경계** — 예: 에이전트 프로세스 내부의 인메모리 regex matcher와 하위 요약기 사이를 흐르는 OCR 텍스트. 파이프라인이 동작하려면 원본이 어딘가에 존재해야 하며, 살균은 그 다음 저장 / 전송 경계에서 적용한다.

2. **사용자가 의도적으로 제출한 저작 콘텐츠** — 버그 리포트, LLM 챗 메시지, 수동 playbook 내용. 사용자가 타이핑해 명시적으로 공유한 텍스트; 살균은 진단 가치를 파괴. 해당 콘텐츠가 2차 경계로 흐르면 (예: chat history → SQLite → sync), 그 2차 경계에서 살균.

3. **크로스-디바이스 sync payload** — 수신자는 동일 사용자가 소유한 다른 디바이스; 전송은 end-to-end 암호화됨 (`sync/sync_crypto.rs` 참조). 여기서 살균하면 sync 기능의 가치 파괴.

4. **Secret projection 경로** — `ProcessEnvSecretProjection` 등은 설계상 의도적으로 비밀 정보를 소비자에게 전달. 누출이 아닌 PII 처리 인프라.

### 예외 문서화

모든 예외 경로는 다음을 포함해야 함:

- 경계 사이트에 `// PII-EXEMPT: <이유>` 주석
- 내부 PII 감사 매트릭스에 예외 근거 명시 행
- 예외가 의도적임을 확인하는 회귀 테스트 (예: raw 텍스트가 살균되지 않고 흐르는지 assert)

조용한 우회(silent bypass)는 허용되지 않음.

## 회귀 테스트

각 fix 사이트는 `src-tauri/tests/pii_sanitization_contract.rs`에 contract test 필수:

1. 알려진 PII를 포함한 입력 생성 (예: `"user@example.com"`)
2. 프로덕션 코드 경로로 라우팅
3. 경계에서의 출력이 예상 marker 토큰(`[EMAIL]`, `[PHONE]`, `[USER]` 등)을 포함함을 assert
4. raw PII가 존재하지 않음을 assert

테스트는 pre-fix `main`에서 실패해야 하고 fix 적용 후 통과해야 함 — 이 패턴이 fix가 이론상이 아닌 실제 gap을 해결함을 증명.

## 결과(Consequences)

- 사용자의 OCR, 클립보드, 접근성 텍스트, LLM 응답이 로컬에 저장되거나 외부로 전송될 때 프라이버시가 유지된다.
- Silent regression은 contract test suite가 포착.
- 새 텍스트 생산 어댑터는 명확한 통합 프로토콜 확보: `PiiSanitizer` 주입 + contract test 추가.

## 변경 프로세스

새 텍스트 생산 어댑터 또는 새 경계 추가 시:

1. 경계 식별 (저장/전송 지점)
2. 위 패턴에 따라 살균 적용
3. Contract test 추가
4. 내부 PII 감사 매트릭스 행 추가
5. 어댑터의 모듈 레벨 문서에서 이 계약 문서 참조
