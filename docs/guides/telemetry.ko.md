[English](./telemetry.md) | [한국어](./telemetry.ko.md)

# 텔레메트리 (Telemetry)

> **Opt-in. 기본 OFF. 기본적으로 프라이빗.**

ONESHIM Rust 클라이언트는 운영 트러블슈팅을 위해 분산 트레이스 span을 OpenTelemetry 콜렉터로 전송할 수 있다. 이 문서는 무엇을 수집하는지, 어떻게 켜고 끄는지, 자체 콜렉터로 보내는 방법, 그리고 콜렉터 쪽에 남는 식별자를 지우는 방법을 다룬다.

## 수집되는 것

텔레메트리가 켜져 있고 `telemetry` Cargo feature가 빌드에 포함된 경우:

- **`tracing` span** (타임스탬프, span 이름, 부모/자식 링크, 숫자 속성). PII 없음. 화면 내용 없음. 키 입력 없음.
- **OpenTelemetry Resource 속성** (모든 span에 부착):
  - `service.name` — 기본값 `oneshim-client`. 사용자가 아닌 바이너리를 식별.
  - `service.instance.id` — 설치당 한 번 최초 opt-in 시 생성하는 UUIDv4. 사용자 식별자에서 파생되지 않는다. 앱 데이터 디렉터리의 `telemetry_instance_id` 파일에 저장 (아래 참조). 콜렉터가 "누가 실행하는지"를 모르는 상태로 같은 설치의 span을 묶을 수 있게 해준다.

`crash_reports`, `usage_analytics`, `performance_metrics` 필드는 config에 예약되어 있지만 현재 릴리스에서는 **와이어링되지 않는다**. 텔레메트리 feature는 span 내보내기만 담당한다.

## 수집되지 않는 것

- 스크린 캡처, OCR 텍스트, Accessibility 트리 내용.
- 채팅 메시지, 파일 내용, 설정값.
- 사용자 식별자, 이메일, 그리고 기존 `PiiFilterLevel` 파이프라인을 거치지 않은 데이터.

## 활성화 방법

기본값은 **OFF**. 켜려면:

1. 설정 → 개인정보 → 텔레메트리 메뉴를 연다.
2. **텔레메트리 활성화**를 켠다.

변경은 몇 초 이내에 반영되며 클라이언트를 재시작할 필요가 없다. 최초 opt-in 시 위에서 설명한 `telemetry_instance_id` 파일이 생성된다.

고급 사용자는 `config.json`을 직접 편집해도 된다:

```json
{
  "telemetry": {
    "enabled": true,
    "otlp_endpoint": null,
    "sample_rate": 1.0,
    "service_name": "oneshim-client"
  }
}
```

설정 파일 위치 (플랫폼별):
- **macOS**: `~/Library/Application Support/oneshim/config.json`
- **Linux**: `~/.config/oneshim/config.json`
- **Windows**: `%APPDATA%/oneshim/config.json`

## 비활성화 방법

`telemetry.enabled`을 `false`로 설정 (UI 토글 또는 `config.json` 편집). 비동기 한 틱 이내에 내보내기가 멈춘다. `telemetry_instance_id` 파일은 의도적으로 남겨둬서 다시 켜면 동일한 식별자를 재사용한다 — [식별자 삭제](#식별자-삭제) 참조.

## 자체 콜렉터로 보내기

세 가지 방법, 우선순위 순 (높은 것이 이김):

1. **명시적 config**: `config.json`의 `telemetry.otlp_endpoint`에 콜렉터의 전체 `/v1/traces` URL을 지정.
2. **환경 변수**: `OTEL_EXPORTER_OTLP_ENDPOINT=https://otel.example.com` — OpenTelemetry 사양에 따라 클라이언트가 `/v1/traces`를 자동으로 붙인다.
3. **기본값**: `http://localhost:4318/v1/traces` (OTLP/HTTP-proto 기본 엔드포인트). 로컬에서 `otel/opentelemetry-collector-contrib` 컨테이너를 띄워 디버깅할 때 유용.

클라이언트는 OTLP over HTTP/proto를 사용한다. gRPC fallback은 현재 노출되지 않는다.

## 컴파일-타임 게이팅

`telemetry.enabled = true`이더라도 바이너리가 `telemetry` Cargo feature 없이 빌드되었다면 익스포터는 아무 동작도 하지 않는다. 기본 릴리스 빌드는 feature **OFF**로 출시되므로, 텔레메트리를 원치 않는 사용자는 바이너리 크기 / 의존성 비용을 전혀 부담하지 않는다. 패키저가 feature를 포함하려면 `cargo build --release --features telemetry -p oneshim-app`로 빌드해야 한다.

## 식별자 삭제

`telemetry_instance_id` 파일에는 UUIDv4가 들어있고 콜렉터는 이것으로 같은 설치의 span을 묶는다. 설치를 지우지 않고 식별자만 재발급하려면:

1. 텔레메트리를 비활성화한다 (이전 UUID를 참조하는 span이 생기지 않도록).
2. 앱 데이터 디렉터리의 `telemetry_instance_id` 파일을 삭제한다:
   - **macOS**: `~/Library/Application Support/oneshim/data/telemetry_instance_id`
   - **Linux**: `~/.local/share/oneshim/telemetry_instance_id`
   - **Windows**: `%LOCALAPPDATA%/oneshim/data/telemetry_instance_id`
3. 텔레메트리를 다시 활성화한다. 새 UUIDv4가 생성되고 `0600` 퍼미션(Unix)으로 기록된다.

전용 `telemetry reset-instance-id` CLI 명령은 이후 릴리스에 제공 예정. 현재는 위 수동 절차가 공식 경로.

## 트러블슈팅

- **"span이 콜렉터에 도달하지 않는다"** — 콜렉터가 지정한 엔드포인트에서 리스닝 중인지, `/v1/traces`에서 OTLP/HTTP-proto를 받아들이는지 확인한다. 빠른 로컬 스모크 테스트: `docker run -p 4318:4318 otel/opentelemetry-collector-contrib:latest`.
- **"텔레메트리 껐는데 아직 데이터가 나간다"** — 실제로는 나가지 않는다. 다만 배치가 이미 전송 중일 수 있다. 새 span 수락은 비동기 한 틱 이내에 멈추고, 진행 중인 HTTP POST는 4초 이내에 완료 또는 타임아웃된다 (shutdown watchdog).
- **"무엇이 전송됐는지 어디서 보나?"** — 익스포터의 warn 수준 실패는 앱의 다른 부분과 같은 tracing subscriber로 로깅된다 (`src-tauri/src/telemetry/otlp.rs::shutdown`의 `warn` 매크로). 디버그 로그는 `RUST_LOG=opentelemetry=debug,oneshim=debug`로 활성화.

## 참고

- 명세: 내부 config telemetry implementation record
- ADR-016 ConfigChangeBus: [`docs/architecture/ADR-016-config-change-bus.md`](../architecture/ADR-016-config-change-bus.md)
- OpenTelemetry 사양 — Resource semantics, OTLP/HTTP transport.
