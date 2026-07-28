[English](./telemetry.md) | [한국어](./telemetry.ko.md)

# 텔레메트리 (Telemetry)

> **설정 기본값은 OFF. 실제 내보내기는 모든 런타임 게이트를 통과해야 함.**

MAEKON Rust 클라이언트는 prerelease 트러블슈팅을 위해 분산 트레이스 span과, 소량의 바운디드(bounded)·비(非)PII 메트릭을 OpenTelemetry 콜렉터로 전송할 수 있다. 이 문서는 무엇을 수집하는지, 어떻게 켜고 끄는지, 자체 콜렉터로 보내는 방법, 그리고 콜렉터 쪽에 남는 식별자를 지우는 방법을 다룬다.

## 서로 다른 네 가지 제어

다음 제어를 하나의 “텔레메트리 ON/OFF” 주장으로 합치지 않는다.

1. **설정 기본값** — 저장되는 `telemetry.enabled`의 기본값은 `false`다.
2. **유효 런타임 상태** — 설정이 활성화되고 유효한 **기능 동의**가 있을 때만 내보내기가 활성화될 수 있다.
3. **빌드 기능** — 바이너리에 `telemetry Cargo feature`가 포함되어야 하며, 기본 release build에는 포함되지 않는다.
4. **진단 내보내기 동의** — 지원 번들은 별도의 로컬 생성 사용자 액션이다. Runtime log는 기본 제외되며, 사용자가 검토하고 명시적으로 보낼 때만 공유된다.

한 제어를 변경해도 다른 제어가 활성화되었다는 뜻은 아니다.

## 수집되는 것

텔레메트리가 켜져 있고 `telemetry` Cargo feature가 빌드에 포함된 경우, OpenTelemetry 경로는 다음을 내보낼 수 있다.

- **`tracing` span** (타임스탬프, span 이름, 부모/자식 링크, 숫자 속성). PII 없음. 화면 내용 없음. 키 입력 없음.
- **OpenTelemetry 메트릭** — 최소한의, 바운디드 카디널리티의, 비PII 인스트루먼트 집합을 OTLP/HTTP `/v1/metrics`로 전송:
  - `maekon.client.heartbeat` — 카운터. 스케줄러 하트비트 틱. 레이블 없음.
  - `maekon.client.scheduler.loop.iterations` — 카운터. 유일한 레이블은 코드에 고정 정의된 루프 이름(작은 닫힌 집합)뿐.
  - `maekon.client.batch_upload.success` / `maekon.client.batch_upload.failure` — 카운터. 유일한 레이블은 코드에 고정 정의된 바운디드 업로드-채널 authority뿐. 전체 URL/경로/쿼리나 사용자 입력에서 파생된 값은 절대 사용하지 않는다.
  - 설계상 사용자별/창 제목별/앱별/세션별/문서별 레이블은 **없다**.
- **OpenTelemetry Resource 속성** (모든 span과 메트릭에 부착 — 두 시그널이 같은 Resource를 공유):
  - `service.name` — 기본값 `maekon-client`. 사용자가 아닌 바이너리를 식별.
  - `service.instance.id` — consent gate를 통과한 최초 exporter activation 때 생성하는 설치별 UUIDv4. 사용자 식별자에서 파생되지 않는다. 앱 데이터 디렉터리의 `telemetry_instance_id` 파일에 저장 (아래 참조). 콜렉터가 "누가 실행하는지"를 모르는 상태로 같은 설치의 텔레메트리를 묶을 수 있게 해준다.

`crash_reports`, `usage_analytics` 필드는 config에 예약되어 있지만 현재 릴리스에서는 **와이어링되지 않는다**. 텔레메트리 feature는 span 및 바운디드 메트릭 내보내기만 담당한다.

## Feature performance sample

Feature performance sample은 OpenTelemetry collector 경로와 별개다. 실제 feature 실행을 감싼 명시적 instrumentation에서만 생성되며, 현재 telemetry consent가 허용될 때 MAEKON 서버의 feature-performance endpoint로 flush된다.

sample contract는 의도적으로 좁다.

- `feature_key`: `local-suggestions`, `sync`처럼 client가 정의한 canonical feature key. 작은 code-defined 집합이며 사용자 입력이 아니다.
- `response_time_ms`: feature 실행 자체의 measured wall-clock duration. flag evaluation latency나 host CPU/memory snapshot이 아니다.
- `timestamp`: measured invocation 완료 시각.
- `total_requests`, `error_count`: 해당 invocation 또는 batch의 bounded counter. `error_count <= total_requests`를 만족해야 한다.

sample에는 user identifier, organization identifier, feature id, document id, raw content, prompt, OCR text, screen content, window title, `success_rate`, `availability`, `error_rate`, `status`, `health_score` 같은 derived health field가 들어가지 않는다.

client는 `feature_key`별 bounded in-memory queue에 sample을 보관한다. consent가 꺼져 있으면 sample은 로컬에서 폐기되고, egress ledger가 설정된 경우 blocked egress로 기록된다. consent가 켜져 있으면 각 upload가 destination `server.feature_performance`로 egress ledger에 audit된다.

## 수집되지 않는 것

- 스크린 캡처, OCR 텍스트, Accessibility 트리 내용.
- 채팅 메시지, 파일 내용, 설정값.
- 사용자 식별자, 이메일, 그리고 기존 `PiiFilterLevel` 파이프라인을 거치지 않은 데이터.
- 공개 feature-performance payload는 server-side identifier나 derived health field를 포함하지 않는다. organization context는 서버가 인증된 request context에서 직접 해석한다.

## 활성화 방법

fresh install에서는 `telemetry.enabled` 설정 기본값이 `false`다. 유효 런타임 상태에서 내보내기가 가능하려면 다음 세 텔레메트리 gate가 모두 열려야 한다.

1. 사용자가 저장 설정을 `telemetry.enabled=true`로 변경해야 한다.
2. 사용자가 `telemetry`에 유효한 기능 동의를 승인해야 한다.
3. 바이너리가 `telemetry` Cargo feature와 함께 빌드되어 있어야 한다.

`"enabled": false`를 저장한 기존 config 파일은 opt-out 상태로 유지된다. 지원되는 build에서 텔레메트리를 요청하려면 설정 → 개인정보 → 텔레메트리에서 **텔레메트리 활성화**를 켜거나 `config.json`을 직접 편집한다. 이 변경은 기능 동의나 compile-time gate를 우회하지 않는다.

변경은 몇 초 이내에 반영되며 클라이언트를 재시작할 필요가 없다. consent gate를 통과한 최초 activation 때 위에서 설명한 `telemetry_instance_id` 파일이 생성된다.

진단 번들 생성과 공유는 이 toggle에 포함되지 않는다. 진단 요청은
`include_logs=false`가 기본이며, 사용자가 번들 내용을 별도로 선택하고 생성된
artifact를 검토한 뒤 명시적인 지원 경로로 보내야 한다.

고급 사용자는 `config.json`을 직접 편집해도 된다:

```json
{
  "telemetry": {
    "enabled": true,
    "otlp_endpoint": null,
    "sample_rate": 1.0,
    "service_name": "maekon-client"
  }
}
```

설정 파일 위치 (플랫폼별):
- **macOS**: `~/Library/Application Support/maekon/config.json`
- **Linux**: `~/.config/maekon/config.json`
- **Windows**: `%APPDATA%/maekon/config.json`

## 비활성화 방법

`telemetry.enabled`을 `false`로 설정 (UI 토글 또는 `config.json` 편집). 비동기 한 틱 이내에 내보내기가 멈춘다. `telemetry_instance_id` 파일은 의도적으로 남겨둬서 다시 켜면 동일한 식별자를 재사용한다 — [식별자 삭제](#식별자-삭제) 참조.

## 자체 콜렉터로 보내기

세 가지 방법, 우선순위 순 (높은 것이 이김):

1. **명시적 config**: `config.json`의 `telemetry.otlp_endpoint`에 지정. **두 시그널 모두** 이 값을 그대로(verbatim) 사용하므로, 익스포터가 쓰는 시그널별 경로를 받아들이는 콜렉터를 가리켜야 한다.
2. **환경 변수**: `OTEL_EXPORTER_OTLP_ENDPOINT=https://otel.example.com` — OpenTelemetry 사양에 따라 베이스 URL로 취급되며, 클라이언트가 span에는 `/v1/traces`, 메트릭에는 `/v1/metrics`를 붙인다.
3. **기본값**: `http://localhost:4318` (OTLP/HTTP-proto 기본 엔드포인트) — span은 `/v1/traces`, 메트릭은 `/v1/metrics`로 전송. 로컬에서 `otel/opentelemetry-collector-contrib` 컨테이너를 띄워 디버깅할 때 유용.

클라이언트는 OTLP over HTTP/proto를 사용한다. gRPC fallback은 현재 노출되지 않는다.

## 컴파일-타임 게이팅

`telemetry.enabled = true`이더라도 바이너리가 `telemetry` Cargo feature 없이 빌드되었다면 익스포터는 아무 동작도 하지 않는다. 기본 릴리스 빌드는 feature **OFF**로 출시되므로, 텔레메트리를 원치 않는 사용자는 바이너리 크기 / 의존성 비용을 전혀 부담하지 않는다. 패키저가 feature를 포함하려면 `cargo build --release --features telemetry -p maekon-app`로 빌드해야 한다.

## 식별자 삭제

`telemetry_instance_id` 파일에는 UUIDv4가 들어있고 콜렉터는 이것으로 같은 설치의 span을 묶는다. 설치를 지우지 않고 식별자만 재발급하려면:

1. 텔레메트리를 비활성화한다 (이전 UUID를 참조하는 span이 생기지 않도록).
2. 앱 데이터 디렉터리의 `telemetry_instance_id` 파일을 삭제한다:
   - **macOS**: `~/Library/Application Support/maekon/data/telemetry_instance_id`
   - **Linux**: `~/.local/share/maekon/telemetry_instance_id`
   - **Windows**: `%LOCALAPPDATA%/maekon/data/telemetry_instance_id`
3. 텔레메트리를 다시 활성화한다. 새 UUIDv4가 생성되고 `0600` 퍼미션(Unix)으로 기록된다.

전용 `telemetry reset-instance-id` CLI 명령은 이후 릴리스에 제공 예정. 현재는 위 수동 절차가 공식 경로.

## 트러블슈팅

- **"span이 콜렉터에 도달하지 않는다"** — 콜렉터가 지정한 엔드포인트에서 리스닝 중인지, `/v1/traces`에서 OTLP/HTTP-proto를 받아들이는지 확인한다. 빠른 로컬 스모크 테스트: `docker run -p 4318:4318 otel/opentelemetry-collector-contrib:latest`.
- **"텔레메트리 껐는데 아직 데이터가 나간다"** — 실제로는 나가지 않는다. 다만 배치가 이미 전송 중일 수 있다. 새 span/메트릭 수락은 비동기 한 틱 이내에 멈추고, meter provider는 no-op으로 리셋되며, span·meter provider 모두 전용 스레드에서 종료되어 진행 중인 HTTP POST는 4초 이내에 완료 또는 타임아웃된다 (shutdown watchdog는 두 시그널 모두에 적용).
- **"무엇이 전송됐는지 어디서 보나?"** — 익스포터의 warn 수준 실패는 앱의 다른 부분과 같은 tracing subscriber로 로깅된다 (`src-tauri/src/telemetry/otlp.rs::shutdown`의 `warn` 매크로). 디버그 로그는 `RUST_LOG=opentelemetry=debug,maekon=debug`로 활성화.

## 참고

- Feature performance contract: [Feature performance sample](#feature-performance-sample)
- ADR-016 ConfigChangeBus: [`docs/architecture/ADR-016-config-change-bus.md`](../architecture/ADR-016-config-change-bus.md)
- OpenTelemetry 사양 — Resource semantics, OTLP/HTTP transport.
